# Lab2 - 重写 sys_get_time 和 sys_trace，mmap 和 munmap 匿名映射 syscall

## 重写 sys_get_time 和 sys_trace 思路

与 ch3 相比，多了虚拟地址，所以不能直接使用 user 程序传递的裸指针去直接写，sys_trace 也是类似情况吗，故 ch4 中先使用 `current_user_token()` 获得当前 user 程序的 token 从而去访问此 task PT，再使用 `PT.translate()` 拿到 PTE，从而获取 PPN，剩下的 `VA & 0xfff` 获取 offset 即可组装出 PA，再使用 PhysAddr 的 `get_mut()` 读写真实物理地址来存放数据。
跨页检查依靠 偏移 + TimeVal size 时是否超出 `PAGE_SIZE` 来判断。

## mmap 和 munmap 匿名映射 syscall 实现思路

基本检查就是整数溢出、除以 PAGE_SIZE 对齐检查、有效地址检查，这一块两个 syscall 一致，`sys_mmap` 中交叉检查依靠通过 token 拿到 task PT 后，使用 `PT.translate()` 于 start_vpn(floor 的 start_va)..end_vpn(ceil 的 end_va) 遍历中检查 Some(PTE) 是否存在以及他 V flag 是否有效来判断是否在申请区域内已经存在被分配的内存，即是否发生交叉，不存在交叉则接着构建 MapPermission 然后使用 `insert_framed_area()` 给当前 task 插入此内存。

munmap 则是补加了 `MemorySet::unmap_current_user_range()` 以直接访问到当前 task 的 PT 和 `Vec<MapArea>`，检查删除目标 start_vpn 和 end_vpn 范围之间的所有 PTE 有效（逻辑类似 mmap），随后检查 flag 必须为 U、检查独占，remaining 一个新的 Vec 用于填充保留后的 MapAreas，遍历中 drop 和 unmap 要取消分配的内存。

## 简答问题

1. SV39 组成：SV39的页表项（PTE）是一个64位的整数，其核心组成部分包括物理页号（PPN，占44位） 和标志位（Flags，占10位），此外还有为内核预留的RSW位（2位）。除了 RSW 位，其它位如下：

- V (Valid, 位0)：有效位。为1表示该PTE有效，为0表示无效，访问无效PTE会触发缺页异常。
- R/W/X (Read/Write/eXecute, 位1-3)：分别控制页面的读、写、执行权限。当R=W=X=0时，该PTE不是叶子节点，而是指向下一级页表的指针；否则为叶子PTE，提供实际的地址映射。
- U (User, 位4)：用户位。为1表示该页面允许在用户态（U-mode） 下访问；为0则仅限内核态（S-mode） 访问，是实现内存隔离的关键。
- G (Global, 位5)：全局位。为1表示该映射是全局的，在地址空间切换时，对应的TLB条目不应被刷新，可提升性能。
- A (Accessed, 位6)：访问位。硬件在页面被访问（读/写/取指）后将其置1，供操作系统实现页面置换算法时参考。
- D (Dirty, 位7)：脏位。硬件在页面被写入后将其置1，表示页面内容已被修改，在换出时需要写回磁盘。

2. 缺页，也就是访问 Page 失败/异常，抛出 PageFault，可以发生在 MMU 检查 PTE 时发现 V=0 PTE 无效、权限不足（U 模式下写只读的 Page）、地址访问非法，外加简答里面提到的 Lazy 加载策略也会触发缺页。
缺页发生时，会 trap 进 os，此时重要的寄存器（借助了 ChatGPT 查找并翻译官方手册 见 1、2、3）：

- stval (Supervisor Trap Value)：最关键的寄存器，硬件会自动将导致缺页的虚拟地址（如出错的指令地址或数据访问地址）写入其中，操作系统据此判断是哪一次访存出了问题。
- sepc (Supervisor Exception Program Counter)：记录触发异常的指令地址。缺页处理完成后，操作系统通常需要重新执行该指令，因此需要sepc来定位。
- scause (Supervisor Cause)：记录陷入原因，其值会标明是哪种页错误（如Load/Store/Instruction Page Fault），帮助操作系统区分异常类型。
- sstatus (Supervisor Status)：记录处理器状态，其中SPP位记录了陷入前的特权等级（用户态或内核态）。

- 懒加载在业务开发前端技术里面也有，vue router 里就有（扯远了），这么干的好处很明显，正如举得例子那样子，10G 总申请，但先只给他实际用的那点，可以更合理的分配硬件资源，系统整体响应速度应该也会提升。

- Sv39 的 4KB 页表下，一个页表是 4 x 1024 / 8 = 512 个 PTE，10G 连续内存的页表内存占用估算应该是：

每个PTE映射4KiB页面，10G 就要 10 x 2^30 / 2^12 = 2.5 x 10^6 个 PTE，每个 PTE 64 位即 8 字节， 2.5 x 10^6 x 8字节 约等于 20MB

- Lazy 加载实现思路：

1. 记录信息：在进程的MemorySet结构中，记录下已分配但尚未映射的虚拟地址范围、权限和后备存储（如文件偏移）等信息，但不建立实际的PTE映射（或建立V=0的PTE）。

2. 缺页处理：当访问这些地址触发缺页异常时，在trap_handler中检查stval中的地址是否在记录的Lazy区域内，如果是，则分配一个物理页框，从磁盘（或直接清零）读取数据填充，填写对应的PTE（设置PPN、权限位V=1等），返回后重新执行引发异常的指令。

- Swap策略中页面失效在PTE上的表现
当页面被换出到磁盘（Swap Out）时，该页面在内存中的映射失效。在PTE上通常表现V位清零（V=0）：表示该PTE无效，访问会触发缺页异常。

RSW位或PPN复用：由于V=0，PTE的PPN区域（及RSW位）可以被操作系统用来存储该页在磁盘交换区中的位置（如扇区号），以便在Swap In时能找回数据。

3. 双页表与单页表

- 单页表情况下，如何更换页表？
在单页表模式下，用户态和内核态共用同一张页表，其根页表物理地址保存在satp寄存器中。更换页表的时机是进程切换（调度到另一个用户进程）时。操作系统会将新进程的页表根地址写入satp，并通常伴随TLB刷新（或使用ASID避免全局刷新-deepseek补充）。

- 单页表情况下，如何控制用户态无法访问内核页面？
通过PTE的U标志位实现。内核页面的PTE中，U位被设置为0，表示该页面仅限S-mode（内核态）访问。当CPU运行在U-mode时，若试图访问U=0的页面，MMU会检测到权限不匹配并触发缺页异常，从而阻止非法访问。

- 单页表的优势
性能更高：进行系统调用或中断陷入内核时，无需切换页表（即无需修改satp），避免了因页表切换导致的TLB刷新和上下文开销。

实现更简单：操作系统无需维护两套页表，也无需处理“跳板页”（trampoline）等复杂机制，降低了内核开发的复杂度。

- 双页表实现下何时更换页表？若写单页表OS，会选择何时更换？
双页表下的更换时机：在用户态与内核态相互切换时更换。从用户态进入内核态（如系统调用、中断）时，切换为内核页表；从内核态返回用户态时，切换为用户页表。这是KPTI（内核页表隔离）的核心机制。

单页表OS的更换时机：仅在进程切换（调度）时更换页表。因为用户态和内核态共用一张页表，陷入内核时无需切换，只有切换到另一个完全不同的用户地址空间时，才需要将satp更新为新进程的页表根地址。

## 硬件 TLB 中内容更新方法 note

```rust
    /// Change page table by writing satp CSR Register.
    pub fn activate(&self) {
        let satp = self.page_table.token(); // frame_alloc() <-  StackFrameAllocator::alloc() ，操作系统维护分配
        unsafe {
            satp::write(satp);
            asm!("sfence.vma");
        }
    }
```

## 声明

使用了 DeepSeek LLM 辅助理解汇编代码和 RISC-V 官方英文手册，但未使用 dsh 等 Agent 工具进行代码生成。

## 荣誉准则

1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与 以下各位 就（与本次实验相关的）以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：

主流大模型，如 DeepSeek、ChatGPT

2. 此外，我也参考了 以下资料 ，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：

1: https://docs.riscv.org/reference/isa/v20260120/priv/supervisor.html
2: https://www.cnblogs.com/lordtianqiyi/p/17968242#3
3: https://acore-guide.sjtu.app/page_table.html
《2027 操作系统考研复习指导-王道著》
《RISC-V指令集手册第二卷-特权架构-中文版-20240411》

3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。 我清楚地知道，从以上方面获得的信息在一定程度上降低了实验难度，可能会影响起评分。

4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。 我未曾也不会向他人（含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。 我提交至本实验的评测系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。 我清楚地知道，以上情况均为本课程纪律所禁止，若违反，对应的实验成绩将按“-100”分计。