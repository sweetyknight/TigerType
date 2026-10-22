//! 固定槽位发现与宿主客户端；只在激活阶段连接，不在逐键回调重试或启动后台。
use crate::{
    ClientState, Error, HEADER_LEN, Result, decode, encode, frame_len,
    pipe::{Pending, Pipe, Runtime},
};
use std::{
    os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle},
    path::Path,
    time::Instant,
};
use tigerrime_contracts::protocol::{Command, Message, Reply, Request, Route};
use windows_sys::Win32::{
    Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::WaitForSingleObject,
};
pub const HOST_SLOTS: usize = 16;

pub struct Client {
    request: Pipe,
    notifications: Pipe,
    pending: Option<Pending>,
    buffered: Vec<u8>,
    completion: OwnedHandle,
    state: ClientState,
    pub generation: u64,
    pub host: u64,
    sequence: u64,
    closed: bool,
}
impl Client {
    pub fn notification_event(&self) -> BorrowedHandle<'_> {
        self.completion.as_handle()
    }
    /// 仅管理工具可请求停机；服务端仍核对调用进程映像，不把登录 SID 当作停机授权。
    pub fn stop_backend(&mut self, deadline: Instant) -> Result<()> {
        if self.closed {
            return Err(Error::Closed);
        }
        let result = (|| {
            self.request
                .send_frame(&encode(&Message::Stop)?, deadline)?;
            if decode(&self.request.receive_frame(deadline)?)? != Message::Stopped {
                return Err(Error::Invalid("停止后台回复不符"));
            }
            Ok(())
        })();
        self.cancel();
        result
    }
    pub fn connect(
        runtime: &Runtime,
        expected_image: &Path,
        namespace: &str,
        deadline: Instant,
    ) -> Result<Self> {
        for slot in 0..HOST_SLOTS {
            let request = runtime.connect_image(
                &format!("{namespace}-request-{slot}"),
                expected_image,
                deadline,
            );
            let mut request = match request {
                Ok(pipe) => pipe,
                Err(Error::Io(error)) if matches!(error.raw_os_error(), Some(2 | 231)) => continue,
                Err(error) => return Err(error),
            };
            let mut notifications = runtime.connect_image(
                &format!("{namespace}-notice-{slot}"),
                expected_image,
                deadline,
            )?;
            if !request.peer()?.same_process(notifications.peer()?)? {
                return Err(Error::Invalid("两条通道不属于同一后台"));
            }
            request.send_frame(&encode(&Message::Hello)?, deadline)?;
            let Message::Welcome { generation, host } = decode(&request.receive_frame(deadline)?)?
            else {
                return Err(Error::Invalid("缺少后台握手"));
            };
            let pending = notifications.start_read()?;
            return Ok(Self {
                request,
                notifications,
                pending: Some(pending),
                buffered: Vec::new(),
                completion: runtime.completion_event().try_clone_to_owned()?,
                state: ClientState::new(generation)?,
                generation,
                host,
                sequence: 0,
                closed: false,
            });
        }
        Err(Error::Busy)
    }
    pub fn request(&mut self, session: u64, command: Command, deadline: Instant) -> Result<Reply> {
        if self.closed {
            return Err(Error::Closed);
        }
        self.sequence = self.sequence.checked_add(1).ok_or(Error::Closed)?;
        let request = Request {
            route: Route {
                generation: self.generation,
                request_id: self.sequence,
                session_id: session,
            },
            command,
        };
        match self.request.transact(&mut self.state, request, deadline) {
            Ok(response) => Ok(response.reply),
            Err(error) => {
                self.cancel();
                Err(error)
            }
        }
    }
    /// 通知是独立流，允许多个完整通知合包，但每条通知和未消费缓冲都严格有界。
    pub fn poll_notification(&mut self) -> Result<Option<Message>> {
        if self.closed {
            return Err(Error::Closed);
        }
        let result = self.poll_inner();
        if result.is_err() {
            self.cancel();
        }
        result
    }
    fn buffered_notification(
        buffered: &mut Vec<u8>,
        host: u64,
        generation: u64,
    ) -> Result<Option<Message>> {
        if buffered.len() >= HEADER_LEN {
            let size = frame_len(buffered[..HEADER_LEN].try_into().unwrap())?;
            if size > 1024 {
                return Err(Error::Invalid("宿主通知过长"));
            }
            if buffered.len() >= size {
                let message = decode(&buffered[..size])?;
                buffered.drain(..size);
                if let Message::Notification { token, .. } = &message
                    && token.host == host
                    && token.generation == generation
                {
                    return Ok(Some(message));
                }
                return Err(Error::Invalid("通知宿主或代次不符"));
            }
        }
        Ok(None)
    }
    fn poll_inner(&mut self) -> Result<Option<Message>> {
        if let Some(message) =
            Self::buffered_notification(&mut self.buffered, self.host, self.generation)?
        {
            return Ok(Some(message));
        }
        if self.pending.is_none() {
            self.pending = Some(self.notifications.start_read()?);
        }
        if let Some((bytes, count)) = self.pending.as_mut().unwrap().try_complete()? {
            self.pending.take();
            if count == 0 {
                return Err(Error::Closed);
            }
            if self.buffered.len() + count > 32 * 1024 {
                return Err(Error::Invalid("通知缓冲超限"));
            }
            self.buffered.extend_from_slice(&bytes[..count]);
            let message =
                Self::buffered_notification(&mut self.buffered, self.host, self.generation)?;
            // 一次只领取一个完成块；后续分片由下一次完成事件唤醒，避免同步完成递归增长。
            self.pending = Some(self.notifications.start_read()?);
            return Ok(message);
        }
        Ok(None)
    }
    pub fn notification(&mut self, deadline: Instant) -> Result<Message> {
        loop {
            if Instant::now() >= deadline {
                self.cancel();
                return Err(Error::TimedOut);
            }
            if let Some(message) = self.poll_notification()? {
                if Instant::now() >= deadline {
                    self.cancel();
                    return Err(Error::TimedOut);
                }
                return Ok(message);
            }
            let millis = deadline
                .saturating_duration_since(Instant::now())
                .as_nanos()
                .div_ceil(1_000_000)
                .min((u32::MAX - 1) as u128) as u32;
            // SAFETY: Client 持有完成事件；虚假/请求通道唤醒只会重查，不刷新绝对期限。
            match unsafe { WaitForSingleObject(self.completion.as_raw_handle(), millis) } {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => {
                    self.cancel();
                    return Err(Error::TimedOut);
                }
                _ => {
                    self.cancel();
                    return Err(std::io::Error::last_os_error().into());
                }
            }
        }
    }
    pub fn cancel(&mut self) {
        self.closed = true;
        self.state.cancel();
        self.request.cancellation().cancel();
        self.notifications.cancellation().cancel();
        self.pending.take();
        self.buffered.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigerrime_contracts::candidate::CandidateToken;
    #[test]
    fn notification_stream_handles_fragments_coalescing_and_rejects_wrong_identity_or_size() {
        let message = Message::Notification {
            token: CandidateToken {
                host: 3,
                generation: 5,
                session: 1,
                focus: 2,
                revision: 7,
            },
            reset: false,
        };
        let frame = encode(&message).unwrap();
        let mut buffer = Vec::new();
        for byte in &frame[..frame.len() - 1] {
            buffer.push(*byte);
            assert_eq!(
                Client::buffered_notification(&mut buffer, 3, 5).unwrap(),
                None
            );
        }
        buffer.push(*frame.last().unwrap());
        buffer.extend_from_slice(&frame);
        assert_eq!(
            Client::buffered_notification(&mut buffer, 3, 5).unwrap(),
            Some(message.clone())
        );
        assert_eq!(
            Client::buffered_notification(&mut buffer, 3, 5).unwrap(),
            Some(message)
        );
        assert!(buffer.is_empty());
        assert!(Client::buffered_notification(&mut frame.clone(), 4, 5).is_err());
        assert!(Client::buffered_notification(&mut frame.clone(), 3, 6).is_err());
        let mut oversized = frame[..HEADER_LEN].to_vec();
        oversized[8..12].copy_from_slice(&5000u32.to_le_bytes());
        assert!(Client::buffered_notification(&mut oversized, 3, 5).is_err());
        assert!(
            Client::buffered_notification(&mut encode(&Message::Hello).unwrap(), 3, 5).is_err()
        );
    }
}
