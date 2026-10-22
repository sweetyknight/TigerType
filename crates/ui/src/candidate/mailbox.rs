//! 候选画面是有界最新值缓存，不是按键/提交队列；替换旧画面不能丢输入或提交。
use std::{
    io,
    os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle},
    sync::{Arc, Mutex, mpsc::TryRecvError},
};
use tigerrime_contracts::candidate::CandidateFrame;
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForSingleObject},
};
struct State {
    frame: Option<Option<CandidateFrame>>,
    sender: bool,
    receiver: bool,
}
struct Shared {
    state: Mutex<State>,
    event: OwnedHandle,
}
pub struct FrameSender(Arc<Shared>);
pub struct FrameInbox(Arc<Shared>);
pub fn mailbox() -> io::Result<(FrameSender, FrameInbox)> {
    // SAFETY: 未命名、不可继承的自动复位事件，所有权由 Arc 共享的 RAII 对象持有。
    let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
    if event.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: 刚创建的内核句柄只有这一份所有权。
    let event = unsafe { OwnedHandle::from_raw_handle(event) };
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            frame: None,
            sender: true,
            receiver: true,
        }),
        event,
    });
    Ok((FrameSender(shared.clone()), FrameInbox(shared)))
}
fn signal(shared: &Shared) -> io::Result<()> {
    // SAFETY: 调用者在通知期间持有共享事件；不在锁内等待通信或绘制窗口。
    if unsafe { SetEvent(shared.event.as_raw_handle()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
impl FrameSender {
    pub fn send(&self, frame: Option<CandidateFrame>) -> Result<(), String> {
        {
            let mut state = self.0.state.lock().map_err(|_| "候选缓存锁已失效")?;
            if !state.receiver {
                return Err("候选接收端已关闭".into());
            }
            state.frame = Some(frame);
        }
        signal(&self.0).map_err(|e| e.to_string())
    }
}
impl FrameInbox {
    pub fn event(&self) -> BorrowedHandle<'_> {
        self.0.event.as_handle()
    }
    pub fn try_receive(&self) -> Result<Option<CandidateFrame>, TryRecvError> {
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| TryRecvError::Disconnected)?;
        if let Some(frame) = state.frame.take() {
            Ok(frame)
        } else if state.sender {
            Err(TryRecvError::Empty)
        } else {
            Err(TryRecvError::Disconnected)
        }
    }
    pub fn receive(&self) -> Result<Option<CandidateFrame>, String> {
        loop {
            match self.try_receive() {
                Ok(frame) => return Ok(frame),
                Err(TryRecvError::Disconnected) => return Err("候选发送端已关闭".into()),
                Err(TryRecvError::Empty) => {}
            }
            // SAFETY: 本对象持有事件；消费缓存与事件复位无丢唤醒窗口，空闲不轮询。
            if unsafe { WaitForSingleObject(self.0.event.as_raw_handle(), INFINITE) }
                != WAIT_OBJECT_0
            {
                return Err(io::Error::last_os_error().to_string());
            }
        }
    }
}
impl Drop for FrameSender {
    fn drop(&mut self) {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sender = false;
        let _ = signal(&self.0);
    }
}
impl Drop for FrameInbox {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.receiver = false;
        state.frame = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_latest_picture_is_retained_and_disconnect_is_observable() {
        let (sender, inbox) = mailbox().unwrap();
        for _ in 0..100 {
            sender.send(Some(super::super::tests::frame())).unwrap();
        }
        sender.send(None).unwrap();
        assert_eq!(inbox.receive().unwrap(), None);
        assert_eq!(inbox.try_receive(), Err(TryRecvError::Empty));
        drop(sender);
        assert!(inbox.receive().is_err());
    }
}
