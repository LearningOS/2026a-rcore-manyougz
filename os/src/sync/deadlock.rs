use alloc::vec;
use alloc::vec::Vec;

/// 按资源编号和 tid 记录分配情况，mutex、semaphore 各用一份
#[derive(Default)]
pub struct DeadlockDetect {
    available: Vec<usize>,
    allocation: Vec<Vec<usize>>,
    need: Vec<Vec<usize>>,
}

impl DeadlockDetect {
    /// 创建资源时追加一列
    pub fn add_resource(&mut self, id: usize, count: usize) {
        self.available.resize(id + 1, 0);
        self.available[id] = count;
        for row in self.allocation.iter_mut().chain(self.need.iter_mut()) {
            row.resize(id + 1, 0);
        }
    }

    /// 先登记需求，不安全则撤销，本次请求还没有动实际资源
    pub fn request(&mut self, tid: usize, id: usize, enabled: bool) -> bool {
        while self.need.len() <= tid {
            self.need.push(vec![0; self.available.len()]);
            self.allocation.push(vec![0; self.available.len()]);
        }
        self.need[tid][id] += 1;
        if enabled && !self.is_safe() {
            self.need[tid][id] -= 1;
            return false;
        }
        true
    }

    /// lock/down 真正返回后，把需求转为已分配
    pub fn acquire(&mut self, tid: usize, id: usize) {
        self.need[tid][id] -= 1;
        self.allocation[tid][id] += 1;
        self.available[id] -= 1;
    }

    /// unlock/up 时归还资源
    pub fn release(&mut self, tid: usize, id: usize) {
        if let Some(row) = self.allocation.get_mut(tid) {
            // semaphore 允许由没有执行过 down 的线程执行 up
            row[id] = row[id].saturating_sub(1);
        }
        self.available[id] += 1;
    }

    fn is_safe(&self) -> bool {
        let mut work = self.available.clone();
        let mut finish = vec![false; self.need.len()];
        loop {
            let mut progressed = false;
            for i in 0..finish.len() {
                if !finish[i]
                    && self.need[i]
                        .iter()
                        .zip(&work)
                        .all(|(need, work)| need <= work)
                {
                    for (work, allocated) in work.iter_mut().zip(&self.allocation[i]) {
                        *work += allocated;
                    }
                    finish[i] = true;
                    progressed = true;
                }
            }
            if !progressed {
                return finish.iter().all(|finished| *finished);
            }
        }
    }
}
