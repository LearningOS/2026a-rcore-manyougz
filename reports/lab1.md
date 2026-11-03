# Lab1 - sys_trace 实现

## 实现思路

多道批处理系统原型中，所有 app 虽然全部加载于内存，宏观并行但实际微观中还是串行，每一个时刻只有一个 task 在运行，故选择直接在 `TaskCB` 中扩展字段（`pub syscall_num: [isize; MAX_SYSCALL_NUM]`）进行统计，又由于 Rust HashMap 等 KV 数据结构类型在 nostd 模式下需要 alloc crate ，所以采用类似位图的方式来以 index 为 syscall_id(key) 对应位上的 isize 值为该对应 syscall_id 的调用总次数，不过这种方案好像过度增大了 TaskCB 的大小，不知道是否会影响性能。

## 简答作业

以下涉及 RustSBI 的任务均使用 `rustsbi_version = "0.4.1"`

### 问题1

- 报错如下：

```bash
[kernel] PageFault in application, bad addr = 0x0, bad instruction = 0x804003a4, kernel killed it.
[kernel] IllegalInstruction in application, kernel killed it.
[kernel] IllegalInstruction in application, kernel killed it.
```

- 分析：

1. bad_address 故意访问了 0x0，0x0 一般是未映射，类似“防撞墙”，用于捕获 nullptr 引用，在 Windows 中也存在类似的机制（第一次见到是在《深入浅出 Windows API 程序设计 核心编程篇 - 王瑞明著》中）
2. bad_instructions 则是故意 asm! inline 一条 S 模式/内核态的特权指令 sret，程序于 U 模式/用户态执行该指令被 CPU 拒绝而崩溃，无效的指令
3. bad_register 同理类似的 asm! 宏 inline 了 csrr {}, sstatus，尝试读取 S模式/内核态专属的 sstatus 寄存器，被 CPU 拒绝，无效指令

### 问题2

1. 指向 TrapContext 的最低位，`__restore` 中的 sret 很明显是为了从 S 到 U 模式，故常见场景是中断/异常处理完成后切换回 U 模式，另一种场景为启动新进程时为了切换 CPU 特权从 S 模式到 U 模式，受硬件机制限制，必须要有一次 sret 才行，同时新程序拿到的寄存器内容也应该都是 0，那 `__restore` 正好满足这些需求。故这两种场景下使用 `__restore` 比较常见

2. L43-L48，处理了 sstatus(Supervisor Status Register) 用于指定 sret 后 CPU 特权级比应该被设置到什么级别，sepc(Supervisor Exception Program Counter) 用于指定返回后对应的 U 模式用户程序从哪个位置开始执行，sscratch(Supervisor Scratch Register) 内核栈指针与用户栈指针sp 切换的缓冲寄存器，进入内核态时取出 sscratch 内的指针用于内核栈，存入用户栈指针 sp 用于后续返回时正确返回，返回时取出其存储的值用于用户栈指针sp，存入当前内核栈指针。

3. L50-L56，x2 为栈指针 sp，x4 为线程指针 tp。sp 恢复不在这个环节，而是后续读 sscratch 时恢复，即 `csrrw sp, sscratch, sp` 处（DeepSeek v4.1 flash 补充一个原因，说是`直接访问用户态地址可能会触发缺页异常（因为在 S 模式下可能没有映射用户内存，或者受 SUM 位限制`）；tp 线程指针现在没啥用，根据实验书规划，还没有到实现 Thread 的环节，后续应该要保存并恢复 tp，目前先跳过。

4. 如上面在 2 中说的，执行前 sp 还是内核栈指针，执行后取出 sscratch 中的值为用户栈指针，并存入内核栈指针到 sscratch，原子交换 sp 与 sscratch 的值。确保返回到 U 模式后的程序能正常执行，下次进入内核态时内核也能正常运行。

5. 发生切换的为 sret 指令，该指令就是干这个的，具体为将 PC 改为 sepc 中的值即用户程序下一条要跑的的指令地址，随后将 sstatus 设置为寄存器中 SPP 位的值（context.rs 中的 `sstatus.set_spp(SPP::User); //previous privilege mode: user mode` 以及提前设置为 0 即用户态），最后根据 sstatus.SPTE 恢复中断使能状态

6. 类似 4，只不过这个是进入 trap，执行前为用户栈指针 sp，执行后为内核栈指针 sp，sscratch 中执行前为内核栈指针，执行后被换出，变成用户栈指针 sp

7. 参考了《2027 操作系统考研复习指导-王道著》中关于中断部分的内容，ecall、缺页、外中断（INTR、NMI）均会触发从 U 态进入 S 态

## RISC-V 指令集 note

以下为让 LLM 总结的表：

| 类别     | 指令    | 类型 | 格式                                                    | 语义                  |
| :------- | :------ | :--- | :------------------------------------------------------ | :-------------------- |
| 存内存   | `sd`    | 真实 | `sd rs2, offset(rs1)`                                   | 64 位存               |
| 读内存   | `ld`    | 真实 | `ld rd, offset(rs1)`                                    | 64 位读               |
| 跳转     | `jalr`  | 真实 | `jalr rd, offset(rs1)`（也写作 `jalr rd, rs1, offset`） | 寄存器间接跳转 + 链接 |
| 算地址   | `auipc` | 真实 | `auipc rd, imm20`                                       | PC + 高 20 位         |
| 算地址   | `addi`  | 真实 | `addi rd, rs1, imm12`                                   | 加 12 位立即数        |
| 算地址   | `lui`   | 真实 | `lui rd, imm20`                                         | 高 20 位立即数        |
| 特权返回 | `mret`  | 真实 | `mret`（无操作数）                                      | M 模式 trap 返回      |
| 返回     | `ret`   | 伪   | `ret`（无操作数）                                       | `jalr x0, 0(ra)`      |
| 取地址   | `la`    | 伪   | `la rd, symbol`                                         | 符号地址装入寄存器    |
| 调用     | `call`  | 伪   | `call symbol`                                           | 远调用，链接到 `ra`   |

## GUN 汇编器伪操作 note

1. GNU 汇编器伪操作不参与生成 CPU 指令。

| 类别      | 伪操作               | 格式                            | 作用                            |
| :-------- | :------------------- | :------------------------------ | :------------------------------ |
| 段控制    | `.section`           | `.section name[, "flags"]`      | 切换/创建段                     |
| 段控制    | `.text`              | `.text`                         | 切到 `.text` 代码段             |
| 段控制    | `.data`              | `.data`                         | 切到 `.data` 可写数据段         |
| 段控制    | `.bss`               | `.bss`                          | 切到 `.bss` 零初始化段          |
| 段控制    | `.rodata`            | `.rodata`                       | 切到只读数据段                  |
| 符号      | `.globl` / `.global` | `.globl symbol`                 | 声明全局符号，链接器可见        |
| 符号      | `.local`             | `.local symbol`                 | 声明局部符号                    |
| 符号      | `.weak`              | `.weak symbol`                  | 声明弱符号，可被覆盖            |
| 符号      | `.extern`            | `.extern symbol`                | 声明外部符号                    |
| 符号赋值  | `.set`               | `.set symbol, expr`             | 定义/赋值符号，可重复赋值       |
| 符号赋值  | `.equ`               | `.equ symbol, expr`             | 定义常量符号，通常只赋一次      |
| 对齐      | `.align`             | `.align n`                      | 对齐到 2n2*n* 字节（RISC-V 下） |
| 对齐      | `.balign`            | `.balign n`                     | 对齐到 n 字节                   |
| 对齐      | `.p2align`           | `.p2align n`                    | 对齐到 2n2*n* 字节              |
| 空间预留  | `.space`             | `.space size[, fill]`           | 预留 size 字节，可选填充值      |
| 空间预留  | `.skip`              | `.skip size[, fill]`            | 同 `.space`，别名               |
| 空间预留  | `.fill`              | `.fill repeat, size, value`     | 重复填充                        |
| 数据      | `.byte`              | `.byte expr[, ...]`             | 定义 1 字节数据                 |
| 数据      | `.short` / `.half`   | `.short expr[, ...]`            | 定义 2 字节数据                 |
| 数据      | `.word` / `.long`    | `.word expr[, ...]`             | 定义 4 字节数据                 |
| 数据      | `.quad`              | `.quad expr[, ...]`             | 定义 8 字节数据                 |
| 数据      | `.dword`             | `.dword expr[, ...]`            | 定义 8 字节数据（某些目标）     |
| 字符串    | `.ascii`             | `.ascii "str"`                  | 嵌入字符串，不加结尾 0          |
| 字符串    | `.asciz` / `.string` | `.asciz "str"`                  | 嵌入字符串，自动加 `\0`         |
| 嵌入文件  | `.incbin`            | `.incbin "file"`                | 把二进制文件原样嵌入            |
| 嵌入文件  | `.include`           | `.include "file"`               | 汇编期包含另一个汇编源文件      |
| 宏定义    | `.macro`             | `.macro name [param...]`        | 定义汇编宏                      |
| 宏定义    | `.endm`              | `.endm`                         | 结束宏定义                      |
| 宏展开    | `.altmacro`          | `.altmacro`                     | 启用替代宏语法，支持 `%expr`    |
| 重复      | `.rept`              | `.rept count`                   | 重复块 count 次                 |
| 重复      | `.endr`              | `.endr`                         | 结束 `.rept` 块                 |
| 重复      | `.irp`               | `.irp param, arg1, arg2, ...`   | 对参数列表逐项重复              |
| 重复      | `.irpc`              | `.irpc param, chars`            | 对字符逐个重复                  |
| 条件      | `.if`                | `.if expr`                      | 条件汇编                        |
| 条件      | `.ifdef` / `.ifndef` | `.ifdef symbol`                 | 符号是否已定义                  |
| 条件      | `.else` / `.endif`   | `.else` / `.endif`              | 条件分支与结束                  |
| 类型/大小 | `.type`              | `.type symbol, @function`       | 声明符号类型（函数/对象）       |
| 类型/大小 | `.size`              | `.size symbol, .-symbol`        | 声明符号大小                    |
| 目标选项  | `.option`            | `.option rvc` / `.option norvc` | 控制指令集扩展（如压缩指令）    |
| 目标架构  | `.attribute`         | `.attribute arch, "rv64i2p0"`   | 声明目标架构属性                |
| 文件信息  | `.file` / `.loc`     | `.file n "name"`                | 调试信息中的文件/行号           |
| 调试      | `.cfi_*`             | `.cfi_startproc` 等             | 调用帧信息，用于栈展开          |

## global_asm! 宏 vs asm! 宏

以下为让 LLM 总结的表：

| 要做的事                                     | `asm!`                 | `global_asm!` |
| :------------------------------------------- | :--------------------- | :------------ |
| 定义全局入口符号 `_start`                    | 不行（只能在函数体内） | 行            |
| 切换段 `.section .text.entry` / `.bss.stack` | 不行                   | 行            |
| 用 `.space` 在 `.bss` 里分配静态栈           | 不行                   | 行            |
| 用 `.incbin` 嵌入二进制文件                  | 不行                   | 行            |
| 定义 `.global` 符号给链接器和 Rust 用        | 不行                   | 行            |

asm! 是“指令级”嵌入，global_asm! 是“汇编文件级”嵌入。内核入口、启动栈、嵌入用户程序，都属于后者

## 声明

使用了 DeepSeek LLM 辅助理解汇编代码，但未使用 dsh 等 Agent 工具进行代码生成，笔记部分为直接的复制粘贴。

## 荣誉准则

1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与 以下各位 就（与本次实验相关的）以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：

主流大模型，如 DeepSeek、ChatGPT

2. 此外，我也参考了 以下资料 ，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：

《深入浅出 Windows API 程序设计 核心编程篇 - 王瑞明著》
《2027 操作系统考研复习指导-王道著》
《RISC-V指令集手册第二卷-特权架构-中文版-20240411》

3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。 我清楚地知道，从以上方面获得的信息在一定程度上降低了实验难度，可能会影响起评分。

4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。 我未曾也不会向他人（含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。 我提交至本实验的评测系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。 我清楚地知道，以上情况均为本课程纪律所禁止，若违反，对应的实验成绩将按“-100”分计。