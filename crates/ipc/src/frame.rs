//! 单帧分片组装；先验证固定头再复制正文，不把不可信长度当成分配容量。

use crate::{Error, HEADER_LEN, Result, frame_len};

#[derive(Default)]
pub struct FrameAssembler {
    bytes: Vec<u8>,
    expected: Option<usize>,
    finished: bool,
}

impl FrameAssembler {
    /// 每个实例只交付一帧。额外字节或错误头使本实例失效，不尝试猜测帧边界。
    pub fn push(&mut self, bytes: &[u8]) -> Result<Option<Vec<u8>>> {
        if self.finished {
            return Err(Error::Closed);
        }
        let result = self.append(bytes);
        if result.is_err() || matches!(result, Ok(Some(_))) {
            self.finished = true;
            // 出错时释放已收正文；成功时 append 已把整帧所有权移交给调用方。
            self.bytes = Vec::new();
        }
        result
    }

    fn append(&mut self, mut bytes: &[u8]) -> Result<Option<Vec<u8>>> {
        if self.expected.is_none() {
            let count = (HEADER_LEN - self.bytes.len()).min(bytes.len());
            self.bytes.extend_from_slice(&bytes[..count]);
            bytes = &bytes[count..];
            if self.bytes.len() < HEADER_LEN {
                return Ok(None);
            }
            self.expected = Some(frame_len(self.bytes.as_slice().try_into().unwrap())?);
        }
        let expected = self.expected.unwrap();
        if bytes.len() > expected - self.bytes.len() {
            return Err(Error::Invalid("单次请求收到额外帧或尾随数据"));
        }
        self.bytes.extend_from_slice(bytes);
        if self.bytes.len() == expected {
            Ok(Some(std::mem::take(&mut self.bytes)))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MAX_BODY_LEN, encode};
    use tigertype_contracts::protocol::{Command, Message, Request, Route};

    fn test_frame() -> Vec<u8> {
        encode(&Message::Request(Request {
            route: Route {
                generation: 1,
                request_id: 1,
                session_id: 1,
            },
            command: Command::Clear,
        }))
        .unwrap()
    }

    #[test]
    fn every_split_and_byte_at_a_time_deliver_exactly_one_owned_frame() {
        let frame = test_frame();
        for split in 0..frame.len() {
            let mut receiver = FrameAssembler::default();
            assert!(receiver.push(&frame[..split]).unwrap().is_none());
            assert_eq!(receiver.push(&frame[split..]).unwrap(), Some(frame.clone()));
            assert!(matches!(receiver.push(&frame), Err(Error::Closed)));
            assert_eq!(receiver.bytes.capacity(), 0);
        }
        let mut receiver = FrameAssembler::default();
        for (index, byte) in frame.iter().enumerate() {
            let value = receiver.push(std::slice::from_ref(byte)).unwrap();
            assert_eq!(value.is_some(), index + 1 == frame.len());
        }
    }

    #[test]
    fn rejects_bad_headers_before_body_copy_and_releases_partial_buffers() {
        let mut frame = test_frame();
        frame[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        frame.resize(MAX_BODY_LEN + HEADER_LEN + 1, b'x');
        let mut receiver = FrameAssembler::default();
        assert!(receiver.push(&frame[..5]).unwrap().is_none());
        assert!(receiver.push(&frame[5..]).is_err());
        assert_eq!(receiver.bytes.capacity(), 0);
        assert!(matches!(receiver.push(&[]), Err(Error::Closed)));

        let mut receiver = FrameAssembler::default();
        let good = test_frame();
        assert!(receiver.push(&good[..good.len() - 1]).unwrap().is_none());
        assert!(receiver.push(&[0, 0]).is_err());
        assert_eq!(receiver.bytes.capacity(), 0);
    }

    #[test]
    fn a_maximum_frame_is_allowed_but_coalesced_frames_are_not_delivered() {
        let mut frame = test_frame();
        frame[8..12].copy_from_slice(&(MAX_BODY_LEN as u32).to_le_bytes());
        frame.resize(HEADER_LEN + MAX_BODY_LEN, b' ');
        let mut receiver = FrameAssembler::default();
        assert!(receiver.push(&frame[..HEADER_LEN]).unwrap().is_none());
        // 验证头部长度后仍不预分配整个正文，只按实际收到的数据增长。
        assert!(receiver.bytes.capacity() < MAX_BODY_LEN);
        assert_eq!(receiver.push(&frame[HEADER_LEN..]).unwrap(), Some(frame));

        let mut receiver = FrameAssembler::default();
        let mut joined = test_frame();
        joined.extend(test_frame());
        assert!(receiver.push(&joined).is_err());
    }
}
