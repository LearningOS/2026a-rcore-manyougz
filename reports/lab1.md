# Lab1 - sys_trace 实现

## 改动

- 两个新 pub fn: `increase_current_task_syscall_num` 和 `current_task_syscall_num`。
- 在 `config.rs` 中定义了一个新常量 `MAX_SYSCALL_NUM`。
- `impl TaskManager` 追加两个新 fn(&self): `task_happen_syscall` 与 `get_task_syscall_num` 分别为上面的俩个封装的 pub fn 的具体实现。
- 为 TaskControlBlock(太长了，本报告后面统一叫 TaskCB，避免和线程控制块 TCB 撞名) 追加了一个新字段 `pub syscall_num: [isize; MAX_SYSCALL_NUM]` 做为某一个 task 的某一 syscall_id 的调用次数统计字段。

## 思路

多道批处理系统原型中，所有 app 虽然全部加载于内存（我参考的王道 2027 操作系统考研复习书中还有脱机 I/O 的外存受主机调控参与，这里先简述，link_app.S 中是一共加载了 10 个 app），宏观并行但实际微观中还是串行，每一个时刻只有一个 task 在运行，故选择直接在 `TaskCB` 中扩展字段（`pub syscall_num: [isize; MAX_SYSCALL_NUM]`）进行统计，又由于 Rust HashMap 等 KV 数据结构类型在 nostd 模式下需要 alloc crate ，所以采用类似位图的方式来以 index 为 syscall_id(key) 对应位上的 isize 值为该对应 syscall_id 的调用总次数，不过这种方案好像过度增大了 TaskCB 的大小，不知道是否会影响性能。

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
