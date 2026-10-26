//! 子进程收尾的**唯一**一处实现：有界排空。
//!
//! 为什么单独抽出来：这段逻辑在本产品里**出过两次同样的错**。
//!
//! - 第 5 刀（`deliverables::run_shell`）：一个 600ms 的"超时"实际跑了 **31.7 秒**；
//! - 第 14 刀（`engine_run::run`）：超时测试又跑了 **30 秒**。
//!
//! 两次根因相同：收尾时 `reader.join()` 会一直等读线程 → 读线程在等管道 EOF →
//! 而**孙进程**还握着 stdout/stderr。杀掉整棵树本该解决，但在受限环境里
//! 进程树清扫会降级为"只杀 root"（例如 `ps` 不可用），孙代就留下来了。
//!
//! 于是纪律是：**收尾必须按墙钟有界，绝不能 `join()`**。
//! 抽成一处之后，第三次写这种代码时就不必再重新踩一遍。
//!
//! 注：spawn 与读取线程的样板在三处仍有重复 —— 但那部分没出过错，
//! 且各自的数据形态不同（按行 / 按字节）。这里只统一**危险的那一段**。

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// 有界排空：等读取线程收完，但**最多等 `grace`**。
///
/// 返回 `true` = 读线程都收干净了（输出完整）；
/// `false` = 仍有子进程没退出、管道没关，**输出可能不完整** ——
/// 调用方必须把这件事说出来，而不是假装完整。
pub fn drain_bounded(threads: &[&JoinHandle<()>], grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        if threads.iter().all(|t| t.is_finished()) {
            return true;
        }
        if Instant::now() >= deadline {
            // 不等了：线程交给运行时自行回收。**这就是全部要点** ——
            // `join()` 在这里会一直等孙进程放手。
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 收尾时追加给人看的一句话。`drained == false` 时必须说 ——
/// "输出可能不完整"是使用者有权知道的事。
pub fn drain_note(drained: bool) -> &'static str {
    if drained {
        ""
    } else {
        "\n（仍有子进程未退出，输出可能不完整）"
    }
}

/// 默认排空宽限。够普通的正常退出收完，又不至于让超时失真。
pub const DEFAULT_DRAIN_GRACE: Duration = Duration::from_millis(500);

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    /// ★ 核心不变量：**已经结束的线程立刻返回 true**（正常路径不受影响）。
    #[test]
    fn a_finished_reader_returns_immediately() {
        let t = std::thread::spawn(|| {});
        std::thread::sleep(Duration::from_millis(50)); // 让它退完再测
        let start = Instant::now();
        assert!(drain_bounded(&[&t], Duration::from_secs(5)));
        assert!(start.elapsed() < Duration::from_millis(100), "不该等");
        t.join().unwrap();
    }

    /// ★ 核心不变量：**赖着不退的读线程不会拖住墙钟**。
    /// 这正是那两次"假超时"的形状：读线程等着孙进程松开管道。
    #[test]
    fn a_stuck_reader_does_not_block_past_the_grace() {
        let (tx, rx) = mpsc::channel::<()>();
        let done = Arc::new(Mutex::new(false));
        let d2 = done.clone();
        // 模拟"孙进程握着管道"：线程一直不结束，直到我们放行
        let t = std::thread::spawn(move || {
            let _ = rx.recv();
            *d2.lock().unwrap() = true;
        });
        let start = Instant::now();
        let drained = drain_bounded(&[&t], Duration::from_millis(150));
        let elapsed = start.elapsed();
        assert!(!drained, "还没收干净就必须如实返回 false");
        assert!(
            elapsed < Duration::from_millis(600),
            "墙钟必须有界（实测 {elapsed:?}）—— 绝不能 join() 到天荒地老"
        );
        // 放行，避免线程泄漏到其它测试
        let _ = tx.send(());
        t.join().unwrap();
        assert!(*done.lock().unwrap());
    }

    /// 多个读取线程：**全**结束才算收干净。
    #[test]
    fn all_readers_must_finish() {
        let fast = std::thread::spawn(|| {});
        let (tx, rx) = mpsc::channel::<()>();
        let slow = std::thread::spawn(move || {
            let _ = rx.recv();
        });
        std::thread::sleep(Duration::from_millis(50)); // fast 已退，slow 还赖着
        assert!(
            !drain_bounded(&[&fast, &slow], Duration::from_millis(120)),
            "只要还有一个没收完就不能说收干净了"
        );
        let _ = tx.send(());
        std::thread::sleep(Duration::from_millis(50)); // slow 也退完
        assert!(drain_bounded(&[&fast, &slow], Duration::from_millis(200)));
        fast.join().unwrap();
        slow.join().unwrap();
    }

    #[test]
    fn no_readers_counts_as_drained() {
        assert!(drain_bounded(&[], Duration::from_millis(10)));
    }

    #[test]
    fn the_note_is_only_shown_when_output_may_be_incomplete() {
        assert_eq!(drain_note(true), "");
        assert!(drain_note(false).contains("输出可能不完整"));
    }
}
