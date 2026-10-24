//! Process management syscalls
use crate::task::*;
use crate::mm::*;
use crate::config::PAGE_SIZE;
use crate::timer::get_time_us;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    // task token
    let token = current_user_token();
    // 访问 task PageTable
    let task_pt = PageTable::from_token(token);
    // 拿到 value ts 对应的 pte，简写 v_ts
    let v_ts_va = VirtAddr::from(ts as usize);
    let v_ts_page_offset = v_ts_va.page_offset();
    let v_ts_pte = task_pt.translate(v_ts_va.floor()).unwrap();
    // ppn << 12 | va 的业内偏移，拿到真实的物理地址
    let v_ts_pa: PhysAddr = v_ts_pte.ppn().into();
    let v_ts_pa: usize = v_ts_pa.into();
    let v_ts: usize = v_ts_pa | v_ts_page_offset;
    // 检查是不是跨页，跨页的话分别检索对应的 PA
    if v_ts_page_offset + core::mem::size_of::<TimeVal>() > PAGE_SIZE {
        // 复用，写 sec 字段
        let ts: PhysAddr = v_ts.into();
        let ts_sec_ptr: &mut usize = ts.get_mut::<usize>();
        *ts_sec_ptr = us / 1_000_000;

        // 写 usec 字段
        // 基本重复上述步骤，但用 ceil() 取到下一页的 usec
        let v_ts_usec_pte = task_pt.translate(v_ts_va.ceil()).unwrap();
        let v_ts_usec_pa: PhysAddr = v_ts_usec_pte.ppn().into();
        let v_ts_usec_pa: usize = v_ts_usec_pa.into();

        // offset 同步到下一页的偏移
        let usec_offset = VirtAddr::from(v_ts_va.ceil()).page_offset();
        let v_ts_usec: usize = v_ts_usec_pa | usec_offset;

        let ts_usec: PhysAddr = v_ts_usec.into();
        let ts_usec_ptr: &mut usize = ts_usec.get_mut::<usize>();
        *ts_usec_ptr = us % 1_000_000;
    } else {
        let ts: PhysAddr = v_ts.into();

        let ts_ptr: &mut TimeVal = ts.get_mut::<TimeVal>();

        *ts_ptr = TimeVal {
            sec: us / 1_000_000,
            usec: us % 1_000_000,
        };
    }
    0
}

/// 读写当前 task 的一个字节，或查询指定 syscall 的调用次数
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    match trace_request {
        0 | 1 => {
            // task token，访问当前 task 的 PageTable
            let token = current_user_token();
            let task_pt = PageTable::from_token(token);
            let v_id_va = VirtAddr::from(id);

            // 避免 VirtAddr::from 截取低 39 位后访问到另一个地址
            if usize::from(v_id_va) != id {
                return -1;
            }

            let v_id_page_offset = v_id_va.page_offset();
            let v_id_pte = match task_pt.translate(v_id_va.floor()) {
                Some(pte) => pte,
                None => return -1,
            };

            // PTE 有效、用户可见，并且具有本次操作需要的权限
            let mut pte_flags = PTEFlags::V | PTEFlags::U;
            if trace_request == 0 {
                pte_flags |= PTEFlags::R;
            } else {
                pte_flags |= PTEFlags::W;
            }
            if !v_id_pte.flags().contains(pte_flags) {
                return -1;
            }

            // PPN 先转换为物理页起始地址，再拼接页内偏移
            let v_id_pa: PhysAddr = v_id_pte.ppn().into();
            let v_id_pa: usize = v_id_pa.into();
            let v_id: usize = v_id_pa | v_id_page_offset;

            // 只访问一个 u8，不会跨页
            if trace_request == 0 {
                unsafe {
                    let ptr = v_id as *const u8;
                    ptr.read() as isize
                }
            } else {
                unsafe {
                    let ptr = v_id as *mut u8;
                    ptr.write(data as u8);
                }
                0
            }
        }
        2 => current_task_syscall_num(id),
        _ => -1,
    }
}

// YOUR JOB: Implement mmap.
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    trace!("kernel: sys_mmap");

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
        // RISC-V 不支持 W=1、R=0 的叶子 PTE，要追加一个 R
        mem_perm |= MapPermission::W | MapPermission::R;
    }
    if (port & 0b100) != 0 {
        mem_perm |= MapPermission::X;
    }

    // 追加内存
    change_task_memory_set(start_va, end_va, mem_perm);
    
    0
}

// YOUR JOB: Implement munmap.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel: sys_munmap");

    // 基础检查
    let start_va = VirtAddr::from(start);
    // start_va 对齐检查
    if start % PAGE_SIZE != 0 {
        return -1;
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

    if TaskManager::unmap_current_user_range(start_vpn, end_vpn) {
       0
    } else {
        -1
    }
}
/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
