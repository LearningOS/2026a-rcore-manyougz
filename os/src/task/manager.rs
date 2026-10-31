//!Implementation of [`TaskManager`]
use super::TaskControlBlock;
use crate::config::BIG_STRIDE;
use crate::sync::UPSafeCell;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use lazy_static::*;
///A array of `TaskControlBlock` that is thread-safe
pub struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

/// 按 stride 选择下一个 task
impl TaskManager {
    ///Creat an empty TaskManager
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
        }
    }
    /// Add process back to ready queue
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }
    /// 从就绪队列取出 stride 最小的 task
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        if self.ready_queue.is_empty() {
            return None;
        }

        // 先拿队首当候选，再暴力扫一遍，stride 一样则保持原来的先后顺序
        let mut min_idx = 0;
        let mut min_stride = self.ready_queue[0].inner_exclusive_access().stride;
        for (idx, task) in self.ready_queue.iter().enumerate().skip(1) {
            let stride = task.inner_exclusive_access().stride;
            if stride < min_stride {
                min_idx = idx;
                min_stride = stride;
            }
        }

        // 统一减去最小 stride，大小关系不变，同时避免后续累加溢出
        for task in self.ready_queue.iter() {
            let mut inner = task.inner_exclusive_access();
            inner.stride -= min_stride;
        }

        let task = self.ready_queue.remove(min_idx).unwrap();
        {
            let mut inner = task.inner_exclusive_access();
            // 被选中的 stride 已经归零，增加本轮对应的 pass
            let pass = BIG_STRIDE / inner.priority;
            inner.stride += pass;
        }
        // 释放 inner 的借用，再把 task 交给调度器
        Some(task)
    }
}

lazy_static! {
    /// TASK_MANAGER instance through lazy_static!
    pub static ref TASK_MANAGER: UPSafeCell<TaskManager> =
        unsafe { UPSafeCell::new(TaskManager::new()) };
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    //trace!("kernel: TaskManager::add_task");
    TASK_MANAGER.exclusive_access().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    //trace!("kernel: TaskManager::fetch_task");
    TASK_MANAGER.exclusive_access().fetch()
}
