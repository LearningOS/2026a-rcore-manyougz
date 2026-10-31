# Lab3 - 适配 sys_get_time、sys_mmap、sys_unmmap 以及 sys_spawn 新 syscall、stride 调度算法

## 适配 sys_get_time、sys_mmap、sys_unmmap

核心实现逻辑基本没变化，无非是直接在 TCB 结构下用 self 读写 TCBInner 来完成 map 映射，感觉进一步结构化后更简便了

## sys_spawn 新 syscall、stride 调度算法

`sys_spawn` 实现比想象中简单，先类似 exec 利用 `translated_str()` 和 `get_app_data_by_name()` 读取到 elf data，再交给 `TCB::new()` 拿新的 TCB，随后再用 `current_task()` 拿一下 father_task 与 new_task 互绑一下父子关系即可返回 pid。

stride 调度实现主要在 `manager.rs` 的时间片轮转 `fn fetch()` 那里加了暴力扫描当前 ready_queue task 队列，从中选择目前  stride 最小的 task（默认当前 0 号位为 mini，也就是加调度之前应该直接拿到时间片的 task），将其移除队列的同时交给调度器，完成调度，同时每轮减去最小值，防止整数溢出。

## 简答问题（参考文献见 1）

- p1 还是无法执行，p2 执行后 stride 会加上 pass，即 p2.stride = 260，然后溢出，变成 4，而 4 < 255，p1 饿死。

- stride 的增量公式是 pass = BigStride / priority。如果所有进程的优先级 priority >= 2，那么单次执行的步长 pass <= BigStride / 2。
这意味着在任意时刻，所有进程的 stride 值都集中在一个宽度不超过 BigStride / 2 的区间内，当发生溢出时，一个处于 BigStride 附近的值加上 pass 后，会回绕到 0 附近。因为两个真实的 stride 差值绝对值不会超过 BigStride / 2，我们完全可以通过计算两者的带模差值来辨别谁在逻辑上更小： 若 A - B（按无符号回绕计算）得到的结果是一个非常大的数（大于 BigStride / 2），说明 A 在溢出前比 B 大，A 发生了回绕，逻辑上 A < B。通过限制优先级，保证了回绕前后的值不会混淆。

- 补全代码：

```rust
use core::cmp::Ordering;

struct Stride(u64);

impl PartialOrd for Stride {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        // 计算带回绕的差值
        let diff = self.0.wrapping_sub(other.0);
        
        // 假设两个 Stride 永远不会相等，因此不考虑 diff == 0 的情况
        // BigStride 为 u64 的最大值，BigStride / 2 即 u64::MAX / 2
        if diff > u64::MAX / 2 {
            // 差值大于一半，说明 self 其实在逻辑上小于 other（发生了溢出回绕）
            Some(Ordering::Less)
        } else {
            // 差值小于等于一半，说明 self 在逻辑上大于 other
            Some(Ordering::Greater)
        }
    }
}

impl PartialEq for Stride {
    fn eq(&self, other: &Self) -> bool {
        false // 题目假设永远不相等
    }
}
```

## 声明

使用了 DeepSeek LLM 辅助理解汇编代码和 RISC-V 官方英文手册，但未使用 dsh 等 Agent 工具进行代码生成。

## 荣誉准则

1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与 以下各位 就（与本次实验相关的）以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：

主流大模型，如 DeepSeek、ChatGPT

2. 此外，我也参考了 以下资料 ，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：

1：https://oslab.space/2026-spring/exp-4/ch-2
《RISC-V指令集手册第二卷-特权架构-中文版-20240411》

3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。 我清楚地知道，从以上方面获得的信息在一定程度上降低了实验难度，可能会影响起评分。

4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。 我未曾也不会向他人（含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。 我提交至本实验的评测系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。 我清楚地知道，以上情况均为本课程纪律所禁止，若违反，对应的实验成绩将按“-100”分计。