//! 表情包制作和改字共用的生图名额：同时最多 2 个在跑（同一个 Key），其余按先后排在同一条队列里。
//! 任务本身各归各的列表，这里只管谁排在谁前面、轮到谁。只在内存里，不存盘。
//!
//! 用法：自己的列表一变（入队、取消排队的、跑完让出名额）就调 `freed`；`freed` 叫醒登记过的每一家，
//! 各家在回调里 `take` 到拿不到为止，再把自己的列表发出去（别家的进出也会改变 `ahead`）。
//! 锁的先后：先锁自己的任务列表，再锁名额；调 `freed` 时两把都不能占着，回调里不要再调 `freed`。

use std::sync::{Mutex, MutexGuard, PoisonError};

use tauri::AppHandle;

/// 同时在跑的最多几个（两家合计）
const MAX_RUNNING: usize = 2;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Who {
    Meme,
    #[allow(dead_code)] // 改字的任务接入后删
    Edit,
}

#[derive(Clone, PartialEq)]
pub struct Slots {
    running: usize,
    /// 在排队的，先来的在前。号由各家自己发，会重，所以带着是谁的
    waiting: Vec<(Who, u64)>,
}

impl Slots {
    pub const fn new() -> Self {
        Self {
            running: 0,
            waiting: Vec::new(),
        }
    }

    /// 排到队尾（重试也是重新入队）
    pub fn enqueue(&mut self, who: Who, id: u64) {
        self.waiting.push((who, id));
    }

    /// 去掉在排队的；在跑的不动，名额等 `release`
    pub fn remove(&mut self, who: Who, id: u64) {
        self.waiting.retain(|&t| t != (who, id));
    }

    /// 排头是自己的、且在跑的不到 2 个时取出它，占一个名额
    pub fn take(&mut self, who: Who) -> Option<u64> {
        let &(head, id) = self.waiting.first()?;
        if head != who || !self.ready() {
            return None;
        }
        self.waiting.remove(0);
        self.running += 1;
        Some(id)
    }

    /// 有名额，也有排队的
    fn ready(&self) -> bool {
        self.running < MAX_RUNNING && !self.waiting.is_empty()
    }

    /// 跑完了（取消了的等请求回来），让出一个名额：每次 `take` 对应一次
    pub fn release(&mut self) {
        debug_assert!(self.running > 0, "让出的名额比占的多");
        self.running = self.running.saturating_sub(1);
    }

    /// 排在它前面、还在排队的个数，两家的都算；它不在排队时为 0
    pub fn ahead(&self, who: Who, id: u64) -> usize {
        self.waiting
            .iter()
            .position(|&t| t == (who, id))
            .unwrap_or(0)
    }
}

static SLOTS: Mutex<Slots> = Mutex::new(Slots::new());
static ON_FREE: Mutex<Vec<fn(&AppHandle)>> = Mutex::new(Vec::new());

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn slots() -> MutexGuard<'static, Slots> {
    lock(&SLOTS)
}

/// 登记名额或队列变了时的回调，每家启动时登记一次。没登记的那家若排了队，会一直挡着后面的
pub fn on_free(f: fn(&AppHandle)) {
    lock(&ON_FREE).push(f);
}

/// 名额或队列变了：叫醒登记过的每一家（不占着锁）
pub fn freed(app: &AppHandle) {
    let fs = lock(&ON_FREE).clone();
    settle(&SLOTS, fs.len(), |i| fs[i](app));
}

/// 轮流叫 `n` 家，直到最后一次变化之后都叫过了：一家拿走排头，可能露出先前叫过的那家。
/// 变化是自己引起的、又没有能跑的了，就不用再叫它自己（免得把同一份列表发两遍）。
/// 别的线程同时引起的变化，由那个线程自己的 `freed` 负责叫全
fn settle(slots: &Mutex<Slots>, n: usize, call: impl Fn(usize)) {
    let mut called = 0;
    for i in (0..n).cycle() {
        let before = lock(slots).clone();
        call(i);
        let now = lock(slots);
        called = if *now == before {
            called + 1
        } else {
            usize::from(!now.ready())
        };
        if called >= n {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Who::{Edit, Meme};

    fn waiting(list: &[(Who, u64)]) -> Slots {
        let mut s = Slots::new();
        for &(w, id) in list {
            s.enqueue(w, id);
        }
        s
    }

    #[test]
    fn shared_two_running() {
        let mut s = waiting(&[(Meme, 1), (Meme, 2), (Edit, 1)]);
        assert_eq!(s.take(Meme), Some(1));
        assert_eq!(s.take(Meme), Some(2));
        assert_eq!(s.take(Edit), None);
        s.release();
        assert_eq!(s.take(Meme), None);
        assert_eq!(s.take(Edit), Some(1));
        assert_eq!(s.take(Edit), None);
    }

    #[test]
    fn fifo_across_both() {
        let order = [(Meme, 1), (Edit, 1), (Meme, 2), (Edit, 2)];
        let mut s = waiting(&order);
        for (w, id) in order {
            // 排头是别人的：有名额也不给
            let other = if w == Meme { Edit } else { Meme };
            assert_eq!(s.take(other), None);
            assert_eq!(s.take(w), Some(id));
            s.release();
        }
        assert_eq!((s.take(Meme), s.take(Edit)), (None, None));
    }

    #[test]
    fn cancel_waiting_does_not_take_slot() {
        // 两家的号会重：只去掉自己的那个
        let mut s = waiting(&[(Meme, 1), (Edit, 1), (Edit, 2), (Meme, 2)]);
        s.remove(Edit, 1);
        assert_eq!(s.take(Meme), Some(1));
        assert_eq!(s.take(Edit), Some(2));
        s.release();
        assert_eq!(s.take(Meme), Some(2));
    }

    #[test]
    fn cancel_running_keeps_slot_until_release() {
        let mut s = waiting(&[(Edit, 1), (Meme, 1), (Meme, 2)]);
        assert_eq!((s.take(Edit), s.take(Meme)), (Some(1), Some(1)));
        s.remove(Edit, 1);
        assert_eq!(s.take(Meme), None);
        s.release();
        assert_eq!(s.take(Meme), Some(2));
    }

    #[test]
    fn ahead_counts_both() {
        let mut s = waiting(&[(Meme, 1), (Edit, 1), (Meme, 2), (Edit, 2)]);
        assert_eq!((s.ahead(Meme, 1), s.ahead(Edit, 2)), (0, 3));
        assert_eq!(s.take(Meme), Some(1));
        // 在跑的不算排在前面；自己不在排队的为 0
        assert_eq!(s.ahead(Meme, 1), 0);
        assert_eq!((s.ahead(Edit, 1), s.ahead(Meme, 2)), (0, 1));
        // 重新入队排到队尾
        s.enqueue(Meme, 1);
        assert_eq!(s.ahead(Meme, 1), 3);
    }

    /// 两家的回调：按登记顺序 Meme、Edit，各自取到拿不到为止；记下跑了哪些、叫了谁
    struct Both {
        slots: Mutex<Slots>,
        ran: Mutex<Vec<(Who, u64)>>,
        calls: Mutex<Vec<Who>>,
    }

    impl Both {
        fn new(list: &[(Who, u64)]) -> Self {
            Self {
                slots: Mutex::new(waiting(list)),
                ran: Mutex::default(),
                calls: Mutex::default(),
            }
        }

        fn s(&self) -> MutexGuard<'_, Slots> {
            self.slots.lock().unwrap()
        }

        fn pump(&self, i: usize) {
            let who = [Meme, Edit][i];
            self.calls.lock().unwrap().push(who);
            loop {
                let got = self.s().take(who);
                let Some(id) = got else { break };
                self.ran.lock().unwrap().push((who, id));
            }
        }

        fn freed(&self) {
            settle(&self.slots, 2, |i| self.pump(i));
        }

        fn ran(&self) -> Vec<(Who, u64)> {
            std::mem::take(&mut self.ran.lock().unwrap())
        }

        fn calls(&self) -> Vec<Who> {
            std::mem::take(&mut self.calls.lock().unwrap())
        }
    }

    /// 后登记的拿走排头，露出先登记那家的：要回头再叫它
    #[test]
    fn slots_wake_goes_back_to_first() {
        let b = Both::new(&[(Edit, 1), (Meme, 1), (Edit, 2)]);
        b.freed();
        assert_eq!(b.ran(), [(Edit, 1), (Meme, 1)]);
        b.s().release();
        b.freed();
        assert_eq!(b.ran(), [(Edit, 2)]);
    }

    /// 移除排头的等待任务后，后面别家的任务会被叫到
    #[test]
    fn slots_wake_after_head_removed() {
        let b = Both::new(&[(Meme, 1), (Edit, 1), (Meme, 2)]);
        b.s().remove(Meme, 1);
        b.freed();
        assert_eq!(b.ran(), [(Edit, 1), (Meme, 2)]);
    }

    /// 每家都叫到（哪怕没有它能跑的：前面的个数变了要重发列表），没有变化就不重复叫
    #[test]
    fn slots_wake_everyone_once() {
        let b = Both::new(&[(Meme, 1)]);
        b.freed();
        assert_eq!(b.calls(), [Meme, Edit]);
        // 别家拿走了排头，回头再叫先前那家
        b.s().enqueue(Edit, 1);
        b.freed();
        assert_eq!(b.calls(), [Meme, Edit, Meme]);
        // 名额满了也都叫
        b.s().enqueue(Meme, 2);
        b.freed();
        assert_eq!(b.calls(), [Meme, Edit]);
        assert_eq!(b.ran(), [(Meme, 1), (Edit, 1)]);
    }

    /// 回调里再入队（给别家的、给自己的）不死锁，入的也接得上
    #[test]
    fn slots_callback_may_enqueue() {
        for who in [Meme, Edit] {
            let b = Both::new(&[(Meme, 1)]);
            let once = std::cell::Cell::new(false);
            settle(&b.slots, 2, |i| {
                b.pump(i);
                if i == 1 && !once.replace(true) {
                    b.s().enqueue(who, 2);
                }
            });
            assert_eq!(b.ran(), [(Meme, 1), (who, 2)]);
        }
    }

    /// 排头那家没登记：不会叫个没完
    #[test]
    fn slots_wake_stops_at_unregistered() {
        let b = Both::new(&[(Meme, 1), (Edit, 1), (Meme, 2)]);
        settle(&b.slots, 1, |i| b.pump(i));
        assert_eq!(b.ran(), [(Meme, 1)]);
        settle(&b.slots, 0, |i| b.pump(i));
    }

    /// 几个线程同时让出、取消、入队：停下来时没有“有名额、排头却没人叫”的，也没多让
    #[test]
    fn slots_no_lost_wake_across_threads() {
        for _ in 0..200 {
            let b = &Both::new(&[(Meme, 1), (Meme, 2), (Edit, 1), (Edit, 2), (Meme, 3)]);
            b.freed();
            assert_eq!(b.ran(), [(Meme, 1), (Meme, 2)]);
            let gate = &std::sync::Barrier::new(4);
            let acts: [fn(&mut Slots); 4] = [
                Slots::release,
                Slots::release,
                |s| s.remove(Edit, 1),
                |s| s.enqueue(Edit, 3),
            ];
            std::thread::scope(|t| {
                for act in acts {
                    t.spawn(move || {
                        gate.wait();
                        act(&mut b.s());
                        b.freed();
                    });
                }
            });
            let s = b.s();
            assert_eq!((s.running, s.ready()), (MAX_RUNNING, false));
            assert_eq!(b.ran().len(), 2);
        }
    }
}
