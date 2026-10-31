//! Process management syscalls
//!
use alloc::sync::Arc;

use crate::config::PAGE_SIZE;
use crate::{
    fs::{open_file, OpenFlags},
    mm::{
        copy_to_user, translated_refmut, translated_str, MapPermission, PageTable, VirtAddr,
        VirtPageNum,
    },
    task::{
        add_task, current_task, current_user_token, exit_current_and_run_next,
        suspend_current_and_run_next, TaskControlBlock,
    },
    timer::get_time_us,
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

pub fn sys_exit(exit_code: i32) -> ! {
    trace!("kernel:pid[{}] sys_exit", current_task().unwrap().pid.0);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    //trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

pub fn sys_getpid() -> isize {
    trace!("kernel: sys_getpid pid:{}", current_task().unwrap().pid.0);
    current_task().unwrap().pid.0 as isize
}

pub fn sys_fork() -> isize {
    trace!("kernel:pid[{}] sys_fork", current_task().unwrap().pid.0);
    let current_task = current_task().unwrap();
    let new_task = current_task.fork();
    let new_pid = new_task.pid.0;
    // modify trap context of new_task, because it returns immediately after switching
    let trap_cx = new_task.inner_exclusive_access().get_trap_cx();
    // we do not have to move to next instruction since we have done it before
    // for child process, fork returns 0
    trap_cx.x[10] = 0;
    // add new task to scheduler
    add_task(new_task);
    new_pid as isize
}

pub fn sys_exec(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_exec", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let all_data = app_inode.read_all();
        let task = current_task().unwrap();
        task.exec(all_data.as_slice());
        0
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    //trace!("kernel: sys_waitpid");
    let task = current_task().unwrap();
    // find a child process

    // ---- access current PCB exclusively
    let mut inner = task.inner_exclusive_access();
    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let child = inner.children.remove(idx);
        // confirm that child will be deallocated after being removed from children list
        assert_eq!(Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        // ++++ temporarily access child PCB exclusively
        let exit_code = child.inner_exclusive_access().exit_code;
        // ++++ release child PCB
        *translated_refmut(inner.memory_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel:pid[{}] sys_get_time", current_task().unwrap().pid.0);
    let us = get_time_us();
    let time = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };
    // 和 fstat 一样按字节写回，字段本身跨页时也能分别找到对应 PA
    let data = unsafe {
        core::slice::from_raw_parts(
            &time as *const TimeVal as *const u8,
            core::mem::size_of::<TimeVal>(),
        )
    };
    if copy_to_user(current_user_token(), ts as *mut u8, data) {
        0
    } else {
        -1
    }
}

/// YOUR JOB: Implement mmap.
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    trace!("kernel:pid[{}] sys_mmap", current_task().unwrap().pid.0);
    // 其余为必须 0，无意义的权限申请
    if port & !0x7 != 0 || port & 0x7 == 0 {
        return -1;
    }

    // 0 长度申请什么
    if len == 0 {
        return 0;
    }

    let start_va = VirtAddr::from(start);
    // start_va 对齐检查
    if start % PAGE_SIZE != 0 {
        return -1;
    }

    // 整数溢出检查
    let end = match start.checked_add(len) {
        Some(end) => end,
        None => return -1,
    };
    let end_va = VirtAddr::from(end);
    let last_va = VirtAddr::from(end - 1);

    // 由于 VirtAddr::from 后截取低 39 位，不能据此判断输入是否合法
    // 改为检查实际访问的首尾地址是否为 Sv39 规范地址
    // 并防止区间跨越两个规范地址区域之间的无效地址范围
    if usize::from(start_va) != start
        || usize::from(last_va) != end - 1
        || ((start ^ (end - 1)) & (1usize << 38)) != 0
    {
        return -1;
    }

    // 检查 [start, start + len) 中是否存在已经被映射的页
    let page_table = PageTable::from_token(current_user_token());

    let start_vpn = start_va.floor();
    let end_vpn = end_va.ceil();

    for vpn_raw in start_vpn.0..end_vpn.0 {
        let vpn = VirtPageNum::from(vpn_raw);

        if let Some(pte) = page_table.translate(vpn) {
            // 当前 translate 返回 Some 时只是有，但不一定是为有效，无效检查
            if pte.is_valid() {
                return -1;
            }
        }
    }

    // 构建 perm
    // 应用程序申请的，肯定 U 模式下要能访问
    let mut mem_perm = MapPermission::U;

    if (port & 0b001) != 0 {
        mem_perm |= MapPermission::R;
    }
    if (port & 0b010) != 0 {
        // 保留申请的权限，不能额外开放 R；RISC-V 上 W=1、R=0 会触发缺页异常
        mem_perm |= MapPermission::W;
    }
    if (port & 0b100) != 0 {
        mem_perm |= MapPermission::X;
    }

    // 追加内存
    // 改成拿 TCB 实例再改其 inner
    current_task()
        .unwrap()
        .change_current_memoery_set(start_va, end_va, mem_perm);

    0
}

/// YOUR JOB: Implement munmap.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_munmap", current_task().unwrap().pid.0);
    // 基础检查
    let start_va = VirtAddr::from(start);
    // start_va 对齐检查
    if start % PAGE_SIZE != 0 {
        return -1;
    }
    if len == 0 {
        return 0;
    }
    let end = match start.checked_add(len) {
        Some(end) => end,
        None => return -1,
    };
    let last_va = VirtAddr::from(end - 1);

    // 检查 Sv39 规范
    if usize::from(start_va) != start
        || usize::from(last_va) != end - 1
        || ((start ^ (end - 1)) & (1usize << 38)) != 0
    {
        return -1;
    }

    let start_vpn = start_va.floor();
    let end_vpn = VirtAddr::from(end).ceil();

    // 同上，校验逻辑无变化
    if current_task()
        .unwrap()
        .unmap_current_user_range(start_vpn, end_vpn)
    {
        0
    } else {
        -1
    }
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel:pid[{}] sys_sbrk", current_task().unwrap().pid.0);
    if let Some(old_brk) = current_task().unwrap().change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

/// YOUR JOB: Implement spawn.
/// HINT: fork + exec =/= spawn
pub fn sys_spawn(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_spawn", current_task().unwrap().pid.0);

    // 拿 elf data
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let data = app_inode.read_all();
        let f_task = current_task().unwrap();
        let new_task = Arc::new(TaskControlBlock::new(data.as_slice()));
        let new_task_id = new_task.pid.0 as isize;
        // 搭建父子关系，完事释放 child_inner 和 father_inner 的借用
        {
            let mut child_inner = new_task.inner_exclusive_access();
            child_inner.parent = Some(Arc::downgrade(&f_task));
            let mut father_inner = f_task.inner_exclusive_access();
            father_inner.children.push(new_task.clone());
        }
        // 注册 new task
        add_task(new_task);
        new_task_id
    } else {
        -1
    }
}

// YOUR JOB: Set task priority.
pub fn sys_set_priority(prio: isize) -> isize {
    trace!(
        "kernel:pid[{}] sys_set_priority",
        current_task().unwrap().pid.0
    );
    if prio <= 1 {
        return -1;
    }
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    // 只改优先级，不清空已经累计的 stride；下一轮调度时重新计算 pass
    inner.priority = prio as usize;
    prio
}
