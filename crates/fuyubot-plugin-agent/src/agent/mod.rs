//! Agent 运行时：网关调用、提示词、单次 / 循环推理、防环。
//!
//! 只依赖宿主 ABI 与 `core` / `session` 的纯类型，不直接碰 TDLib。按"一层一件事"拆开：
//!
//! - `gateway`：把一轮对话翻译成网关（`fengyegf.llm`）的一条共享服务调用；
//! - `completion`：把三种协议的上游响应统一成一段文本 + 用量（纯解析，可单测）；
//! - `tools`：宿主工具表 → 三种协议的工具定义，并同步执行模型发起的调用；
//! - `toolcall`：工具调用 / 结果回填的协议形状（纯函数，可单测）；
//! - `prompt`：系统提示 + 最近 N 条历史 + 本轮输入；
//! - `guard`：去重 / 限速 / bot 连击深度（纯内存状态机）；
//! - `engine`：取历史、调网关、工具循环、（循环模式）多轮迭代、落历史。
//!
//! 对外路径保持稳定：`GatewayClient` / `Completion` / `PromptBuilder` / `Guard` /
//! `GuardVerdict` / `AgentEngine`。
//!
//! 两个容易踩的约定：
//!
//! 1. 网关是**软依赖**：服务可能比本插件晚加载、也可能运行期卸载，所以
//!    `GatewayClient::available` 每次都现查宿主，不缓存。
//! 2. 网关调用是同步阻塞的（宿主把控制权直接转给提供方插件的方法），只能由 worker
//!    线程或工具 / 服务处理器发起；在宿主回调里调会卡住整个接收循环。

mod completion;
mod engine;
mod gateway;
mod guard;
mod prompt;
mod toolcall;
mod tools;

pub use completion::Completion;
pub use engine::AgentEngine;
pub use engine::StreamOutcome;
pub use gateway::GatewayClient;
pub use guard::{Guard, GuardVerdict};
pub use prompt::PromptBuilder;
pub use toolcall::ToolCall;
