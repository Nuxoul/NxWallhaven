//! 令牌桶限流器
//!
//! wallhaven 限制 45 请求/分钟。超限返回 429。
//! 本实现是线程安全的，多个工作线程共享同一个桶。

use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct TokenBucket {
    inner: Mutex<Inner>,
    capacity: f64,
    refill_per_sec: f64,
}

struct Inner {
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    /// `capacity`: 桶容量；`per_minute`: 每分钟补充量
    pub fn new(capacity: u32, per_minute: u32) -> Self {
        Self {
            inner: Mutex::new(Inner {
                tokens: capacity as f64,
                last: Instant::now(),
            }),
            capacity: capacity as f64,
            refill_per_sec: per_minute as f64 / 60.0,
        }
    }

    /// wallhaven 默认：45/分钟
    pub fn wallhaven() -> Self {
        Self::new(45, 45)
    }

    /// 尝试立即取一个令牌，取不到返回需要等待的时长
    pub fn try_acquire(&self) -> Result<(), Duration> {
        let mut g = self.inner.lock().unwrap();
        let now = Instant::now();
        if now < g.last {
            return Err(g.last.duration_since(now));
        }
        let elapsed = now.duration_since(g.last).as_secs_f64();
        g.tokens = (g.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        g.last = now;

        if g.tokens >= 1.0 {
            g.tokens -= 1.0;
            Ok(())
        } else {
            let need = 1.0 - g.tokens;
            let wait = need / self.refill_per_sec;
            Err(Duration::from_secs_f64(wait.max(0.001)))
        }
    }

    /// 阻塞直到拿到令牌（供工作线程使用）
    pub fn acquire(&self) {
        loop {
            match self.try_acquire() {
                Ok(()) => return,
                Err(wait) => std::thread::sleep(wait),
            }
        }
    }

    /// 收到 429 时主动惩罚：清空令牌，强制冷却
    pub fn penalize(&self, cooldown: Duration) {
        let mut g = self.inner.lock().unwrap();
        g.tokens = 0.0;
        g.last = Instant::now() + cooldown;
    }

    /// 当前可用令牌（供 UI 显示）
    pub fn available(&self) -> f64 {
        let mut g = self.inner.lock().unwrap();
        let now = Instant::now();
        if now < g.last {
            return 0.0;
        }
        let elapsed = now.duration_since(g.last).as_secs_f64().max(0.0);
        g.tokens = (g.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        g.last = now;
        g.tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_full() {
        let b = TokenBucket::new(45, 45);
        assert!((b.available() - 45.0).abs() < 0.5);
    }

    #[test]
    fn consumes_tokens() {
        let b = TokenBucket::new(3, 3);
        assert!(b.try_acquire().is_ok());
        assert!(b.try_acquire().is_ok());
        assert!(b.try_acquire().is_ok());
        // 第 4 次应失败
        assert!(b.try_acquire().is_err());
    }

    #[test]
    fn err_reports_positive_wait() {
        let b = TokenBucket::new(1, 60); // 每秒补 1 个
        assert!(b.try_acquire().is_ok());
        match b.try_acquire() {
            Err(w) => {
                assert!(w.as_secs_f64() > 0.0);
                assert!(w.as_secs_f64() <= 1.1);
            }
            Ok(_) => panic!("should have been rate limited"),
        }
    }

    #[test]
    fn penalize_zeroes_bucket() {
        let b = TokenBucket::new(45, 45);
        b.penalize(Duration::from_millis(200));
        assert!(b.try_acquire().is_err());
    }

    #[test]
    fn polling_does_not_cancel_server_cooldown() {
        let b = TokenBucket::new(45, 45);
        b.penalize(Duration::from_secs(30));
        let deadline = b.inner.lock().unwrap().last;
        assert!(b.try_acquire().unwrap_err() >= Duration::from_secs(29));
        assert_eq!(b.available(), 0.0);
        assert_eq!(b.inner.lock().unwrap().last, deadline);
        assert!(b.try_acquire().unwrap_err() >= Duration::from_secs(29));
    }

    #[test]
    fn is_thread_safe() {
        use std::sync::Arc;
        let b = Arc::new(TokenBucket::new(10, 600));
        let mut handles = vec![];
        let counter = Arc::new(Mutex::new(0));
        for _ in 0..8 {
            let b = b.clone();
            let c = counter.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..3 {
                    if b.try_acquire().is_ok() {
                        *c.lock().unwrap() += 1;
                    }
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // 初始 10 个 + 期间补充的少量，不应超过合理上限
        let n = *counter.lock().unwrap();
        assert!(n >= 10, "should consume at least initial capacity, got {n}");
        assert!(n < 30, "should not exceed capacity+refill wildly, got {n}");
    }
}
