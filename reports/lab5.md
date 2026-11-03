# Lab5 - mutex、semaphore 死锁检测

## 实现思路

线程共用进程的同步资源，故需要在 PCBInner 中分别记录 mutex 和 semaphore 的可用数量、已分配数量和等待需求。

申请时用 Work、Finish 模拟线程完成，不安全则撤销需求返回 -0xDEAD；lock/down 返回后记录分配，unlock/up 时归还。检测开关按进程保存，默认关闭。适配 sys_get_time，复用字节缓冲区翻译处理跨页写回。

## 资源状态记录 note

`Available` 按资源编号记录剩余数量，`Allocation` 和 `Need` 则以 tid 为行、资源编号为列。mutex 的初始数量为 1，semaphore 使用创建时传入的数量，两种资源分开检测。

基本还是按照文档暴力遍历，先把本次请求加进 Need，再复制 Available 为 Work。找到 Need 不超过 Work 的线程后，假设它可以完成，将它已经持有的 Allocation 加回 Work，然后继续找下一个。整轮都没有进展时，检查 Finish 是否全部为 true。整个模拟只改临时变量，不会真的归还资源。

`sys_enable_deadlock_detect` 只负责检查 0、1 参数并修改开关。资源状态在关闭检测时也继续记录，这样后面开启时不需要重新推断已经分配出去的资源。真正的检查放在 `mutex_lock`、`semaphore_down` 申请资源之前，被拒绝的请求只撤销 Need，不进入原来的等待队列。

阻塞前需要先 drop PCBInner 的借用和局部持有的进程引用，重新运行后再记录本次分配。条件变量的 wait 内部也会先释放 mutex、醒来后重新获取，所以这里也同步处理这两步的记录。

## 实验用时及检查情况

约 3 天，每天约 6h，合计约 18h。
 
检查则是以  make -C ci-user test CHAPTER=8 通过为 goal，无其它检查

## 简答问题

### 主线程退出时需要回收的资源及 TCB 引用

0 号线程退出即整个进程退出，故其他线程不应该再被调度。需要处理的资源主要有：

- 各线程的 tid、用户栈和 TrapContext 对应的映射及物理页框。
- 进程共享的代码段、数据段、堆等用户地址空间，以及最后随 MemorySet 析构释放的页表页框。
- 文件描述符表中的引用，最后一个引用释放时，对应文件、管道等对象才会被回收。
- mutex、semaphore、condvar 等同步对象及其等待队列中的引用。
- 各线程的 TCB 和内核栈，以及最后随 PCB 析构回收的 pid 等资源。

这些资源不是必须在同一处全部释放，关键是先阻止线程再次运行，再按所有权关系释放引用。用户资源需要先于整个地址空间回收，当前还在使用的内核栈则必须等线程切换出去后再回收。

TCB 可能在以下位置被引用：

1. PCB 的 tasks 中。进程退出时清空这些引用，普通线程单独退出时则先保留退出码，供 waittid 回收。
2. TaskManager 的就绪队列中。需要主动移除，否则已经结束的线程可能再次被调度。
3. TIMERS 中。需要主动移除，否则定时器到期后可能重新唤醒已经退出的线程。
4. mutex、semaphore、condvar 的等待队列中。这些对象属于进程内部，等待队列的引用随同步对象析构而释放；PCB 尚处于僵尸状态时，这部分回收也可能延后。
5. Processor 的 current 或调度过程中的局部变量中。退出时取出 current，完成切换所需的处理后释放局部引用。TaskManager 的 stop_task 会暂存正在退出的主线程，等它彻底停止使用内核栈后再释放。

这里也涉及 Rust 借用顺序：TaskUserRes 的 Drop 会再次访问 PCBInner，所以框架先把 res 取出放进临时 Vec，释放 PCBInner 的借用后再统一 drop，避免重复借用。进程的退出码等信息先保留在 PCB 中，父进程通过 waitpid 获取后再完成剩余回收。

### Mutex1 和 Mutex2 的区别

Mutex1 的 unlock 会先把 locked 改为 false，再唤醒一个等待线程。被唤醒只意味着进入就绪队列，真正运行前可能有其他线程先拿到这把锁，所以 lock 中需要 loop 重新检查。这个实现可以保证互斥，被唤醒的线程可能反复竞争失败，增加调度开销，也可能出现饥饿。

Mutex2 在存在等待线程时保留 locked = true，将锁直接交给队首的线程。该线程从阻塞处返回后就视为已经持有锁，故 lock 中只用 if，不需要重新竞争。其他线程看到锁仍被占用，会继续进入等待队列；没有等待者时才把 locked 改为 false。

两套 lock、unlock 需要配套使用。若用 Mutex1 的循环配合 Mutex2 的解锁，被唤醒者会看到 locked 仍为 true，再次入队，导致锁无人继续持有却无法释放。若用 Mutex2 的 if 配合 Mutex1 的解锁，新线程可能已经抢到锁，被唤醒者又直接进入临界区，破坏互斥。

## 荣誉准则

1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与 以下各位 就（与本次实验相关的）以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：

主流大模型，如 DeepSeek、ChatGPT

2. 此外，我也参考了 以下资料 ，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：
   1. https://learningos.cn/rCore-Tutorial-Guide/chapter8/1thread-kernel.html
   2. https://learningos.cn/rCore-Tutorial-Guide/chapter8/2lock.html
   3. https://learningos.cn/rCore-Tutorial-Guide/chapter8/3semaphore.html
   4. https://learningos.cn/rCore-Tutorial-Guide/chapter8/4condition-variable.html

3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。 我清楚地知道，从以上方面获得的信息在一定程度上降低了实验难度，可能会影响起评分。

4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。 我未曾也不会向他人（含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。 我提交至本实验的评测系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。 我清楚地知道，以上情况均为本课程纪律所禁止，若违反，对应的实验成绩将按“-100”分计。