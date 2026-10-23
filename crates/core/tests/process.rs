//! 別プロセスでの耐久性・同時読み取りと通信部品の静的検査。
#[path = "process/concurrent.rs"]
mod concurrent;
#[path = "process/crash.rs"]
mod crash;
#[path = "process/no_network.rs"]
mod no_network;
#[path = "process/support.rs"]
mod support;
