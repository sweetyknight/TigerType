//! 容量为一的回复邮箱与内核事件一起持有；不把窗口句柄或指针传给工作线程。
//! 状态（设置热重载、通知区图标显隐）另用只留最新值的交接槽，覆盖旧值、不排队、不因满而失败。
use std::{
    io,
    os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
};
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent};

fn event() -> io::Result<OwnedHandle> {
    // SAFETY: 无命名、不可继承的自动复位事件；成功后立即交给 OwnedHandle 配对释放。
    let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
    if event.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: 此句柄刚创建且尚无其他所有者。
    Ok(unsafe { OwnedHandle::from_raw_handle(event) })
}

/// 只留最新值的交接槽：set 覆盖未取走的旧值并唤醒等待方，take 取走最新值；跨线程共享时包在 Arc 中。
pub struct Latest<T> {
    value: Mutex<Option<T>>,
    event: OwnedHandle,
}
impl<T> Latest<T> {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            value: Mutex::new(None),
            event: event()?,
        })
    }
    pub fn set(&self, value: T) -> Result<(), String> {
        *self.value.lock().map_err(|_| "交接锁已损坏")? = Some(value);
        // SAFETY: self 持有事件句柄；唤醒失败必须报错，不能让新状态静默不生效。
        if unsafe { SetEvent(self.event.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    pub fn take(&self) -> Result<Option<T>, String> {
        Ok(self.value.lock().map_err(|_| "交接锁已损坏")?.take())
    }
    pub fn event(&self) -> BorrowedHandle<'_> {
        self.event.as_handle()
    }
}

pub struct Inbox<T> {
    pub(crate) receiver: Receiver<T>,
    pub(crate) event: Arc<OwnedHandle>,
}
pub struct ReplySender<T> {
    sender: Option<SyncSender<T>>,
    event: Arc<OwnedHandle>,
}
impl<T> Inbox<T> {
    pub fn event(&self) -> BorrowedHandle<'_> {
        self.event.as_handle()
    }
    pub fn try_receive(&self) -> Result<T, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }
    pub fn receive(&self) -> Result<T, mpsc::RecvError> {
        self.receiver.recv()
    }
}
pub fn mailbox<T>() -> io::Result<(ReplySender<T>, Inbox<T>)> {
    let event = Arc::new(event()?);
    let (sender, receiver) = mpsc::sync_channel(1);
    Ok((
        ReplySender {
            sender: Some(sender),
            event: event.clone(),
        },
        Inbox { receiver, event },
    ))
}
impl<T> ReplySender<T> {
    pub fn send(&self, reply: T) -> Result<(), String> {
        self.sender
            .as_ref()
            .ok_or("回复通道已关闭")?
            .try_send(reply)
            .map_err(|e| e.to_string())?;
        // SAFETY: Arc 保证在通知期间事件尚未关闭；事件失败必须报错，不能静默丢回复。
        if unsafe { SetEvent(self.event.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error().to_string());
        }
        Ok(())
    }
}
impl<T> Drop for ReplySender<T> {
    fn drop(&mut self) {
        // 先断开通道再通知 UI，退出/展开时无需分配新通道。
        drop(self.sender.take());
        // SAFETY: Arc 保持事件有效，使线程退出/展开也能唤醒无限等待。
        unsafe {
            SetEvent(self.event.as_raw_handle());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigertype_contracts::ui::Reply;
    use windows_sys::Win32::{Foundation::WAIT_OBJECT_0, System::Threading::WaitForSingleObject};
    #[test]
    fn latest_slot_keeps_only_the_newest_value_and_wakes_once_per_set() {
        let latest = Latest::new().unwrap();
        assert_eq!(latest.take(), Ok(None));
        // 连续设置不排队、不失败：只留最后一个值。
        latest.set(true).unwrap();
        latest.set(false).unwrap();
        // SAFETY: 测试持有事件，等待有明确上限；自动复位后第二次等待超时。
        unsafe {
            assert_eq!(
                WaitForSingleObject(latest.event().as_raw_handle(), 1000),
                WAIT_OBJECT_0
            );
            assert_ne!(
                WaitForSingleObject(latest.event().as_raw_handle(), 0),
                WAIT_OBJECT_0
            );
        }
        assert_eq!(latest.take(), Ok(Some(false)));
        assert_eq!(latest.take(), Ok(None));
    }

    #[test]
    fn bounded_mailbox_wakes_for_reply_and_worker_exit() {
        let (sender, inbox) = mailbox().unwrap();
        sender.send(Reply::Saved(Err("测试".into()))).unwrap();
        assert!(sender.send(Reply::Saved(Err("不得丢弃".into()))).is_err());
        assert_eq!(
            // SAFETY: 测试持有事件，等待有明确上限。
            unsafe { WaitForSingleObject(inbox.event.as_raw_handle(), 1000) },
            WAIT_OBJECT_0
        );
        assert!(inbox.receiver.try_recv().is_ok());
        drop(sender);
        assert_eq!(
            // SAFETY: Inbox 仍持有事件，线程退出必须通知接收方。
            unsafe { WaitForSingleObject(inbox.event.as_raw_handle(), 1000) },
            WAIT_OBJECT_0
        );
        assert!(matches!(
            inbox.receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
}
