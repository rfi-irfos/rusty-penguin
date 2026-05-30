use ternary_core::Trit;

const MAX_PROCS: usize = 16;

/// Trit-state process entry.
/// state: 1 = Pos (Running), 0 = Zero (Ready), 2 = Neg (Blocked)
#[derive(Clone, Copy)]
pub struct ProcEntry {
    pub pid:   u64,
    pub state: u8,
    pub name:  [u8; 16],
}

static mut PROCS:    [Option<ProcEntry>; MAX_PROCS] = [None; MAX_PROCS];
static mut CURRENT:  usize = 0;
static mut NEXT_PID: u64   = 0;

pub fn init() {
    let mut name = [0u8; 16];
    name[..4].copy_from_slice(b"idle");
    unsafe {
        PROCS[0]  = Some(ProcEntry { pid: 0, state: 0 /* Zero=Ready */, name });
        NEXT_PID  = 1;
        CURRENT   = 0;
    }
}

/// Register a new process (called by kernel just before IRETQ).
/// Marks it Pos (Running) and returns its PID.
pub fn register(proc_name: &[u8]) -> u64 {
    unsafe {
        let pid = NEXT_PID;
        NEXT_PID += 1;
        if (pid as usize) < MAX_PROCS {
            let mut name = [0u8; 16];
            let len = proc_name.len().min(15);
            name[..len].copy_from_slice(&proc_name[..len]);
            PROCS[pid as usize] = Some(ProcEntry { pid, state: 1 /* Pos=Running */, name });
            CURRENT = pid as usize;
        }
        pid
    }
}

pub fn current_pid() -> u64 {
    unsafe { PROCS[CURRENT].map_or(0, |p| p.pid) }
}

pub fn yield_() {
    // Wait for the next hardware interrupt (100Hz timer, keyboard, or mouse).
    // This throttles the ring-3 main loop to ≤100 iterations/second instead of
    // spinning at full CPU speed, which caused topbar/cursor flickering.
    unsafe {
        core::arch::asm!("sti", options(nostack));
        core::arch::asm!("hlt", options(nostack));
    }
}

pub fn trit_of(state: u8) -> Trit {
    match state { 1 => Trit::Pos, 2 => Trit::Neg, _ => Trit::Zero }
}

// ─────────────────────────────────────────────────────────────────────────────
// Real scheduler core — Increment 1: cooperative kernel-task context switching.
//
// Roadmap in docs/SCHEDULER.md. This is the foundation for running the desktop
// and a Linux process (fbDOOM) concurrently. Increment 1 only switches between
// kernel-mode tasks in the shared address space (separate kernel stacks) to
// prove the context_switch primitive; it is gated behind the `schedtest` boot
// flag and does not touch the desktop path.
// ─────────────────────────────────────────────────────────────────────────────

const MAX_TASKS:   usize = 4;
const KSTACK_SIZE: usize = 32 * 1024; // 32 KiB kernel stack per task

#[derive(Clone, Copy)]
struct Task {
    rsp:   u64,  // saved kernel stack pointer (valid when this task is suspended)
    used:  bool,
    alive: bool,
}

static mut TASKS: [Task; MAX_TASKS] =
    [Task { rsp: 0, used: false, alive: false }; MAX_TASKS];
static mut KSTACKS: [[u8; KSTACK_SIZE]; MAX_TASKS] = [[0; KSTACK_SIZE]; MAX_TASKS];
static mut CUR_TASK: usize = 0;

/// Save callee-saved registers + rsp of the current task into `*prev`, then load
/// `next` into rsp and restore that task's callee-saved registers. The `ret`
/// resumes wherever the next task last called context_switch (or its entry, for
/// a freshly-spawned task whose stack we primed below).
#[unsafe(naked)]
unsafe extern "C" fn context_switch(prev: *mut u64, next: u64) {
    core::arch::naked_asm!(
        "push rbp", "push rbx", "push r12", "push r13", "push r14", "push r15",
        "mov [rdi], rsp",   // *prev = current rsp
        "mov rsp, rsi",     // rsp = next
        "pop r15", "pop r14", "pop r13", "pop r12", "pop rbx", "pop rbp",
        "ret",
    )
}

/// Prime a freshly-allocated kernel stack so the first switch INTO this task
/// lands at `entry`. Stack layout (low→high, what context_switch pops):
/// [r15][r14][r13][r12][rbx][rbp][entry].
fn spawn(entry: extern "C" fn() -> !) -> usize {
    unsafe {
        for i in 1..MAX_TASKS {
            if !TASKS[i].used {
                let base = core::ptr::addr_of_mut!(KSTACKS[i]) as *mut u8;
                let mut sp = base.add(KSTACK_SIZE) as u64;
                sp &= !0xF;                 // 16-byte align
                sp -= 8; *(sp as *mut u64) = entry as usize as u64;  // ret target
                for _ in 0..6 { sp -= 8; *(sp as *mut u64) = 0; }    // rbp..r15
                TASKS[i] = Task { rsp: sp, used: true, alive: true };
                return i;
            }
        }
        0
    }
}

/// Pick the next alive task after `from` (round-robin). Task 0 (the boot thread)
/// is always considered alive so we can always fall back to it.
fn next_alive(from: usize) -> usize {
    unsafe {
        for off in 1..=MAX_TASKS {
            let i = (from + off) % MAX_TASKS;
            if i == 0 || (TASKS[i].used && TASKS[i].alive) { return i; }
        }
        0
    }
}

/// Cooperative yield: switch to the next runnable task.
pub fn yield_cpu() {
    unsafe {
        let cur = CUR_TASK;
        let nxt = next_alive(cur);
        if nxt == cur { return; }
        CUR_TASK = nxt;
        context_switch(core::ptr::addr_of_mut!(TASKS[cur].rsp), TASKS[nxt].rsp);
    }
}

/// Terminate the current task and switch away permanently (never returns).
fn task_exit() -> ! {
    unsafe {
        TASKS[CUR_TASK].alive = false;
        let cur = CUR_TASK;
        let nxt = next_alive(cur);
        CUR_TASK = nxt;
        // We don't care about saving `cur`'s context — give it a scratch slot.
        let mut scratch = 0u64;
        context_switch(&mut scratch, TASKS[nxt].rsp);
        // Unreachable: a dead task is never switched back to.
        loop { core::arch::asm!("hlt"); }
    }
}

/// Increment-1 boot self-test (cmdline `schedtest`): spawn two cooperative
/// kernel tasks and interleave serial output with the boot thread. Proves the
/// context_switch primitive before we build on it. Returns to normal boot.
pub fn selftest() {
    use crate::serial::write_str;
    write_str("\n[sched] === Increment 1: cooperative context-switch self-test ===\n");
    unsafe {
        TASKS[0] = Task { rsp: 0, used: true, alive: true };
        CUR_TASK = 0;
    }
    spawn(task_a);
    spawn(task_b);
    for _ in 0..3 {
        write_str("[sched] boot-thread\n");
        yield_cpu();
    }
    write_str("[sched] === self-test done: A/B should have interleaved above ===\n\n");
}

extern "C" fn task_a() -> ! {
    for _ in 0..3 { crate::serial::write_str("[sched]   task A\n"); yield_cpu(); }
    task_exit();
}
extern "C" fn task_b() -> ! {
    for _ in 0..3 { crate::serial::write_str("[sched]   task B\n"); yield_cpu(); }
    task_exit();
}

/// Fill `buf` with packed 32-byte PsRecord entries.
/// Layout per record: [u64 pid][u8 state][7 pad][16 name]
/// Returns number of records written.
pub fn fill_ps(buf: *mut u8, max: usize) -> usize {
    let mut count = 0usize;
    unsafe {
        for slot in PROCS.iter() {
            if count >= max { break; }
            let p = match slot { Some(p) => p, None => continue };
            let dst = buf.add(count * 32);
            for (i, b) in p.pid.to_le_bytes().iter().enumerate() { *dst.add(i) = *b; }
            *dst.add(8) = p.state;
            for i in 9..16usize { *dst.add(i) = 0; }
            for (i, b) in p.name.iter().enumerate() { *dst.add(16 + i) = *b; }
            count += 1;
        }
    }
    count
}
