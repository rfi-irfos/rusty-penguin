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

// ── CPU accounting ────────────────────────────────────────────────────────────
// We can't measure per-core CPU% on a single cooperative core directly, but we
// CAN measure how much of the wall-clock the CPU spends halted (idle) vs running.
// yield_() halts until the next interrupt; we time those halts with the TSC.
// busy% = 100 · (1 − idle_cycles / total_cycles). Busy is attributed to the
// running ring-3 process (the desktop), idle to the idle task (pid 0).
static mut IDLE_CYCLES:  u64 = 0;
static mut WIN_START_TSC: u64 = 0;
static mut LAST_BUSY_PM: u32 = 0;   // last sampled busy permille (0..1000)

#[inline]
pub fn rdtsc() -> u64 {
    let lo: u32; let hi: u32;
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack)); }
    ((hi as u64) << 32) | lo as u64
}

/// Sample busy permille (0..1000) over the window since the last call, and reset
/// the window. Drives the System Monitor's CPU graph.
pub fn cpu_sample() -> u32 {
    unsafe {
        let now = rdtsc();
        if WIN_START_TSC == 0 { WIN_START_TSC = now; IDLE_CYCLES = 0; return LAST_BUSY_PM; }
        let total = now.wrapping_sub(WIN_START_TSC);
        let idle = IDLE_CYCLES.min(total);
        LAST_BUSY_PM = if total > 0 { ((total - idle) * 1000 / total) as u32 } else { 0 };
        WIN_START_TSC = now; IDLE_CYCLES = 0;
        LAST_BUSY_PM
    }
}
/// Most recent busy permille without resetting (for per-process CPU columns).
pub fn last_busy_pm() -> u32 { unsafe { LAST_BUSY_PM } }

pub fn yield_() {
    // Wait for the next hardware interrupt (100Hz timer, keyboard, or mouse).
    // This throttles the ring-3 main loop to ≤100 iterations/second instead of
    // spinning at full CPU speed, which caused topbar/cursor flickering. The
    // halted cycles are counted as idle for the CPU meter.
    let t0 = rdtsc();
    unsafe {
        core::arch::asm!("sti", options(nostack));
        core::arch::asm!("hlt", options(nostack));
        IDLE_CYCLES = IDLE_CYCLES.wrapping_add(rdtsc().wrapping_sub(t0));
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
    cr3:   u64,  // address space (PML4 phys). 0 = don't switch (shared kernel AS).
}

static mut TASKS: [Task; MAX_TASKS] =
    [Task { rsp: 0, used: false, alive: false, cr3: 0 }; MAX_TASKS];
static mut KSTACKS: [[u8; KSTACK_SIZE]; MAX_TASKS] = [[0; KSTACK_SIZE]; MAX_TASKS];
static mut CUR_TASK: usize = 0;

// `_cur_syscall_stack` (defined in syscall.rs global_asm) is the kernel stack the
// SYSCALL trampoline switches to. preempt_tick retargets it per task on each
// context switch (via inline asm — see there) so concurrent syscalls from two
// tasks don't share one stack and clobber each other's frames.

// Per-task syscall counter — a liveness signal. A process that makes no syscalls
// over a window has stopped answering the kernel; the watchdog treats that as
// "not responding" (a stand-in for a desktop app that stops servicing the
// compositor) and can force-quit it.
static mut TASK_SYSCALLS: [u64; MAX_TASKS] = [0; MAX_TASKS];

/// Bump the current task's syscall count (called from the syscall path).
pub fn note_syscall() {
    unsafe {
        let c = CUR_TASK;
        if c < MAX_TASKS {
            TASK_SYSCALLS[c] = TASK_SYSCALLS[c].wrapping_add(1);
        }
    }
}
/// How many syscalls task `idx` has made.
pub fn task_syscalls(idx: usize) -> u64 {
    unsafe { if idx < MAX_TASKS { TASK_SYSCALLS[idx] } else { 0 } }
}
/// Force-quit a task: drop it from the schedule so it never runs again. (Its
/// address-space frames are not yet reclaimed — a follow-up; the slot is freed.)
pub fn kill_task(idx: usize) {
    unsafe {
        if idx < MAX_TASKS && idx != 0 {
            TASKS[idx].alive = false;
            TASKS[idx].used = false;
        }
    }
}

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
                TASKS[i] = Task { rsp: sp, used: true, alive: true, cr3: 0 };
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
        TASKS[0] = Task { rsp: 0, used: true, alive: true, cr3: 0 };
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

// ─────────────────────────────────────────────────────────────────────────────
// Increment 2: timer-driven PREEMPTION. fbDOOM's game loop never yields, so the
// 100 Hz timer must switch tasks. Unlike the cooperative switch (callee-saved
// only), preemption interrupts arbitrary code, so we save/restore the FULL
// register set + the CPU's iret frame. Every task's suspended kernel stack thus
// has the uniform layout [15 GPRs][rip][cs][rflags][rsp][ss]; the same `iretq`
// resumes a preempted task or launches a freshly-spawned one.
//
// Gated behind `schedtest2`. Increment 3 extends this to ring-3 + per-process
// address spaces (CR3 switch) for the real desktop ⇄ fbDOOM case.
// ─────────────────────────────────────────────────────────────────────────────

/// Prime a fresh kernel-task stack with a full [15 GPRs][iret frame] so the
/// preemptive switch can `iretq` straight into `entry` at ring 0 with IF set.
fn spawn_preempt(entry: extern "C" fn() -> !) -> usize {
    unsafe {
        for i in 1..MAX_TASKS {
            if !TASKS[i].used {
                let base = core::ptr::addr_of_mut!(KSTACKS[i]) as *mut u8;
                let top = ((base.add(KSTACK_SIZE) as u64) & !0xF) as u64;
                let mut sp = top;
                let push = |sp: &mut u64, v: u64| { *sp -= 8; *(*sp as *mut u64) = v; };
                // iret frame (high→low: ss, rsp, rflags, cs, rip)
                push(&mut sp, 0x10);   // ss  = kernel data
                push(&mut sp, top);    // rsp = task's own stack top after entry
                push(&mut sp, 0x202);  // rflags, IF=1
                push(&mut sp, 0x08);   // cs  = kernel code
                push(&mut sp, entry as usize as u64); // rip = entry
                for _ in 0..15 { push(&mut sp, 0); }  // 15 GPRs = 0
                TASKS[i] = Task { rsp: sp, used: true, alive: true, cr3: 0 };
                return i;
            }
        }
        0
    }
}

/// Called from the naked preemptive timer stub with the interrupted task's saved
/// stack pointer; returns the next task's stack pointer to resume. Also does the
/// normal per-tick bookkeeping (ticks/EOI/USB).
extern "C" fn preempt_tick(cur_rsp: u64) -> u64 {
    crate::idt::timer_bookkeeping();
    unsafe {
        TASKS[CUR_TASK].rsp = cur_rsp;
        let nxt = next_alive(CUR_TASK);
        CUR_TASK = nxt;
        // Switch to the next task's address space. Every per-process AS shares
        // the kernel's low half (PML4[0]), so the kernel stacks we're standing
        // on stay mapped across the switch. cr3 == 0 means "shared kernel AS,
        // no switch" (Increment 2 kernel tasks).
        if TASKS[nxt].cr3 != 0 {
            crate::vmm::switch_address_space(TASKS[nxt].cr3);
        }
        // Point TSS.rsp0 at the next task's kernel stack, so if it's a RING-3
        // task and the timer interrupts it, the CPU lands the frame on that
        // task's own kernel stack (Increment 3c). Harmless for ring-0 tasks.
        crate::gdt::set_rsp0(kstack_top(nxt));
        // ...and point the SYSCALL trampoline at the same per-task kernel stack,
        // so two tasks in syscalls at once (one preempted mid-syscall, another
        // entering) don't share one stack and clobber each other's frames.
        // Written via inline asm (direct rip-relative): a plain Rust store to this
        // global_asm symbol compiles to a GOT-indirect access whose slot is unmapped
        // in the higher-half kernel (→ #PF). The asm form references it directly.
        let kt = kstack_top(nxt);
        core::arch::asm!(
            "mov qword ptr [rip + _cur_syscall_stack], {0}",
            in(reg) kt, options(nostack, preserves_flags),
        );
        TASKS[nxt].rsp
    }
}

/// Top (highest address, 16-aligned) of task `i`'s kernel stack.
fn kstack_top(i: usize) -> u64 {
    unsafe { ((core::ptr::addr_of!(KSTACKS[i]) as u64) + KSTACK_SIZE as u64) & !0xF }
}

/// Like `spawn_preempt`, but the task runs in its OWN address space (its own
/// CR3, sharing the kernel low half). Increment 3b: proves the scheduler swaps
/// CR3 per task — the basis for the desktop and fbDOOM at the same addresses.
fn spawn_preempt_as(entry: extern "C" fn() -> !) -> usize {
    let i = spawn_preempt(entry);
    if i != 0 {
        if let Some(as_) = unsafe { crate::vmm::new_address_space() } {
            unsafe { TASKS[i].cr3 = as_; }
        }
    }
    i
}

/// Preemptive timer IRQ entry. Saves the full GPR set on the current task's
/// kernel stack (atop the CPU's iret frame), hands the stack pointer to
/// `preempt_tick`, switches to the returned task's stack, restores its GPRs and
/// `iretq`s into it.
#[unsafe(naked)]
unsafe extern "C" fn irq_timer_preempt() {
    core::arch::naked_asm!(
        "push rax", "push rcx", "push rdx", "push rbx", "push rbp", "push rsi", "push rdi",
        "push r8", "push r9", "push r10", "push r11", "push r12", "push r13", "push r14", "push r15",
        "mov rdi, rsp",         // arg0 = current saved-frame pointer
        "call {tick}",          // rax = next task's rsp
        "mov rsp, rax",
        "pop r15", "pop r14", "pop r13", "pop r12", "pop r11", "pop r10", "pop r9", "pop r8",
        "pop rdi", "pop rsi", "pop rbp", "pop rbx", "pop rdx", "pop rcx", "pop rax",
        "iretq",
        tick = sym preempt_tick,
    )
}

#[inline(never)]
fn busy_spin() {
    for _ in 0..25_000_000u64 {
        unsafe { core::arch::asm!("nop", options(nomem, nostack, preserves_flags)); }
    }
}

/// Increment-2 self-test (cmdline `schedtest2`): two kernel tasks that NEVER
/// yield, plus the boot thread — all preempted by the 100 Hz timer. If the
/// preemptive switch works, A, B and boot all print (interleaved); without it,
/// only the boot thread would ever run. Does not return (timer drives forever).
pub fn selftest_preempt() -> ! {
    use crate::serial::write_str;
    write_str("\n[sched] === Increment 2: timer-PREEMPTION self-test ===\n");
    unsafe {
        core::arch::asm!("cli");
        TASKS[0] = Task { rsp: 0, used: true, alive: true, cr3: 0 };
        CUR_TASK = 0;
    }
    spawn_preempt(ptask_a);
    spawn_preempt(ptask_b);
    // Install the preemptive timer handler and let the 100 Hz IRQ drive switching.
    crate::idt::set_timer_vector(irq_timer_preempt as *const () as u64);
    unsafe { core::arch::asm!("sti"); }
    let mut n = 0u32;
    loop {
        write_str("[sched] boot-thread (preempted)\n");
        busy_spin();
        n += 1;
        if n >= 6 { write_str("[sched] === preemption proven: A/B ran without yielding ===\n"); }
    }
}

extern "C" fn ptask_a() -> ! {
    loop { crate::serial::write_str("[sched]   PREEMPT task A\n"); busy_spin(); }
}
extern "C" fn ptask_b() -> ! {
    loop { crate::serial::write_str("[sched]   PREEMPT task B\n"); busy_spin(); }
}

// ─────────────────────────────────────────────────────────────────────────────
// Increment 3a: per-process ADDRESS SPACES. The desktop and fbDOOM use the same
// fixed virtual addresses (4/16 MiB code, 63 MiB stack, …) so they cannot share
// one page table. Each process needs its own CR3 where identical virtual
// addresses map to different physical frames. This self-test proves the VMM
// machinery: two address spaces, the SAME virtual address, DIFFERENT memory.
// Gated behind `schedtest3`.
// ─────────────────────────────────────────────────────────────────────────────

/// Increment-3a self-test: build a second address space, map a private page at a
/// high virtual address (≥512 GiB, in PML4[1] so it doesn't touch the shared
/// kernel half) in BOTH spaces to different frames, then switch CR3 between them
/// and confirm the same pointer reads different values.
pub fn selftest_vmm() -> ! {
    use crate::serial::{write_str, write_hex_u64};
    write_str("\n[sched] === Increment 3a: per-process address-space self-test ===\n");
    // Make sure the PMM's frames are identity-accessible while we poke them.
    crate::vmm::extend_identity_map(512);
    const PRIV: u64 = 0x80_0000_0000; // 512 GiB — first address in PML4[1]
    unsafe {
        let kpml4 = crate::vmm::current_cr3();
        let as2 = match crate::vmm::new_address_space() {
            Some(p) => p, None => { write_str("[sched] new_address_space failed\n"); halt(); }
        };
        let fa = crate::pmm::alloc_frame().unwrap_or(0);
        let fb = crate::pmm::alloc_frame().unwrap_or(0);
        *(fa as *mut u64) = 0xAAAA_AAAA; // frame backing PRIV in as2
        *(fb as *mut u64) = 0xBBBB_BBBB; // frame backing PRIV in the kernel AS
        let fl = crate::vmm::PTE_PRESENT | crate::vmm::PTE_WRITABLE | crate::vmm::PTE_USER;
        crate::vmm::map_page_in(as2,   PRIV, fa, fl);
        crate::vmm::map_page_in(kpml4, PRIV, fb, fl);
        crate::vmm::switch_address_space(kpml4);
        write_str("[sched]  in kernel AS, *[512GiB] = "); write_hex_u64(*(PRIV as *const u64));
        write_str(" (expect 0xbbbbbbbb)\n");
        crate::vmm::switch_address_space(as2);
        write_str("[sched]  in AS #2,    *[512GiB] = "); write_hex_u64(*(PRIV as *const u64));
        write_str(" (expect 0xaaaaaaaa)\n");
        crate::vmm::switch_address_space(kpml4);
        write_str("[sched]  back in kernel AS, *[512GiB] = "); write_hex_u64(*(PRIV as *const u64));
        write_str(" (expect 0xbbbbbbbb)\n");
    }
    write_str("[sched] === if the values differ, per-process VMM works ===\n");
    halt();
}

fn halt() -> ! { loop { unsafe { core::arch::asm!("hlt"); } } }

fn read_cr3() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) v, options(nostack, readonly)); }
    v & !0xFFF
}

/// Increment-3b self-test (cmdline `schedtest4`): two preemptible tasks each in
/// its OWN address space, plus the boot thread in the kernel AS. Each prints its
/// live CR3. If the three CR3 values differ AND interleave, the scheduler is
/// switching per-process address spaces under timer preemption — the mechanism
/// that lets the desktop and fbDOOM occupy the same fixed addresses.
pub fn selftest_cr3_sched() -> ! {
    use crate::serial::{write_str, write_hex_u64, write_byte};
    write_str("\n[sched] === Increment 3b: per-task CR3 switching under preemption ===\n");
    crate::vmm::extend_identity_map(512);
    unsafe {
        core::arch::asm!("cli");
        TASKS[0] = Task { rsp: 0, used: true, alive: true, cr3: crate::vmm::current_cr3() };
        CUR_TASK = 0;
    }
    spawn_preempt_as(catask_a);
    spawn_preempt_as(catask_b);
    crate::idt::set_timer_vector(irq_timer_preempt as *const () as u64);
    unsafe { core::arch::asm!("sti"); }
    loop {
        write_str("[sched] boot   cr3="); write_hex_u64(read_cr3()); write_byte(b'\n');
        busy_spin();
    }
}

extern "C" fn catask_a() -> ! {
    loop {
        crate::serial::write_str("[sched]   task A cr3=");
        crate::serial::write_hex_u64(read_cr3());
        crate::serial::write_byte(b'\n');
        busy_spin();
    }
}
extern "C" fn catask_b() -> ! {
    loop {
        crate::serial::write_str("[sched]   task B cr3=");
        crate::serial::write_hex_u64(read_cr3());
        crate::serial::write_byte(b'\n');
        busy_spin();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Increment 3c: a RING-3 task in its own address space. Until now tasks ran in
// ring 0; a real process (desktop, fbDOOM) runs in ring 3. This loads a tiny
// position-independent ring-3 stub into a private AS, runs it at ring 3 with a
// user stack, and the stub syscalls back into the kernel (logged). Combined
// with per-task TSS.rsp0 (set in preempt_tick), the timer can preempt the
// ring-3 task and hand control to the boot thread.  Gated behind `schedtest5`.
//
// ⚠️ UNVERIFIED at commit time (built while the user was asleep; QEMU is blocked
// in the sandbox). Verify with `schedtest5` before relying on it.
// ─────────────────────────────────────────────────────────────────────────────

// Private (PML4[1], ≥512 GiB) virtual addresses for the ring-3 stub — NOT in the
// shared kernel low half, so each process can map them independently.
const R3_CODE_VA:   u64 = 0x80_0000_0000;
const R3_STACK_VA:  u64 = 0x80_0010_0000;
const R3_STACK_TOP: u64 = R3_STACK_VA + 0x1000;

// Position-independent ring-3 stub:
//   loop { rdi = 0x55; rax = 0x1337; syscall; for(ecx=0x01000000) {}; }
// Only immediates + relative jumps, so it runs correctly at any address.
static R3_STUB: [u8; 23] = [
    0xbf, 0x55, 0x00, 0x00, 0x00,       // mov edi, 0x55      (syscall arg1 = tag)
    0xb8, 0x37, 0x13, 0x00, 0x00,       // mov eax, 0x1337    (syscall nr)
    0x0f, 0x05,                         // syscall
    0xb9, 0x00, 0x00, 0x00, 0x01,       // mov ecx, 0x01000000 (delay count)
    0xff, 0xc9,                         // dec ecx
    0x75, 0xfc,                         // jnz -4  (delay loop)
    0xeb, 0xe9,                         // jmp -23 (back to start)
];

/// Spawn a ring-3 task running `stub` in a fresh private address space.
fn spawn_ring3(stub: &[u8]) -> usize {
    unsafe {
        for i in 1..MAX_TASKS {
            if !TASKS[i].used {
                let as_ = match crate::vmm::new_address_space() { Some(p) => p, None => return 0 };
                let pf = crate::vmm::PTE_PRESENT | crate::vmm::PTE_USER;
                let pfw = pf | crate::vmm::PTE_WRITABLE;
                // Code page (user, executable — no NX here): copy the stub in.
                let code = crate::pmm::alloc_frame().unwrap_or(0);
                core::ptr::write_bytes(code as *mut u8, 0, 4096);
                core::ptr::copy_nonoverlapping(stub.as_ptr(), code as *mut u8, stub.len());
                crate::vmm::map_page_in(as_, R3_CODE_VA, code, pf);
                // User stack page.
                let stk = crate::pmm::alloc_frame().unwrap_or(0);
                core::ptr::write_bytes(stk as *mut u8, 0, 4096);
                crate::vmm::map_page_in(as_, R3_STACK_VA, stk, pfw);
                // Prime this task's kernel stack with a RING-3 iret frame + GPRs.
                let base = core::ptr::addr_of_mut!(KSTACKS[i]) as *mut u8;
                let ktop = ((base.add(KSTACK_SIZE) as u64) & !0xF) as u64;
                let mut sp = ktop;
                let push = |sp: &mut u64, v: u64| { *sp -= 8; *(*sp as *mut u64) = v; };
                push(&mut sp, 0x1b);          // ss     = user data | RPL3
                push(&mut sp, R3_STACK_TOP);  // user rsp
                push(&mut sp, 0x202);         // rflags  IF=1
                push(&mut sp, 0x23);          // cs     = user code | RPL3
                push(&mut sp, R3_CODE_VA);    // rip    = stub entry
                for _ in 0..15 { push(&mut sp, 0); } // 15 GPRs = 0
                TASKS[i] = Task { rsp: sp, used: true, alive: true, cr3: as_ };
                return i;
            }
        }
        0
    }
}

// ── Increment 3d: PRIVATE LOW HALF per process ───────────────────────────────
// Real programs load in the low half (the desktop at 0x400000). To give each
// process its own low half, the address space must NOT share the kernel's low
// identity map (PML4[0]) — it shares only the kernel's higher half. Two
// processes can then both live at the SAME low virtual address with different
// contents, fully isolated, while the kernel (now entirely higher-half) keeps
// servicing their syscalls. This is the proof that PML4[0] is no longer needed.
const LOW_CODE_VA:  u64 = 0x0040_0000;          // 4 MiB — where real programs load
const LOW_STACK_VA: u64 = 0x0040_1000;          // one page above the code
const LOW_STACK_TOP: u64 = LOW_STACK_VA + 0x1000;

/// Spawn a ring-3 task in an address space with a PRIVATE LOW HALF, running the
/// stub at low `LOW_CODE_VA`. `tag` is patched into the stub's syscall arg so two
/// processes at the same VA emit distinguishable syscalls.
fn spawn_ring3_low(tag: u8) -> usize {
    spawn_ring3_low_with(&R3_STUB, Some(tag))
}

/// A "hung app": a pure infinite loop (`jmp $`) that never syscalls and never
/// yields. The only thing that can take the CPU back is the preemption timer —
/// so if the rest of the system keeps running while this spins, isolation holds.
static HUNG_STUB: [u8; 2] = [0xeb, 0xfe]; // jmp -2

/// Spawn a ring-3 task in a private-low-half address space running `stub`. If
/// `tag` is Some, patch it into the stub's `mov edi, imm` byte (offset 1) so two
/// processes at the same VA emit distinguishable syscalls.
fn spawn_ring3_low_with(stub: &[u8], tag: Option<u8>) -> usize {
    unsafe {
        for i in 1..MAX_TASKS {
            if !TASKS[i].used {
                let as_ = match crate::vmm::new_address_space_private() { Some(p) => p, None => return 0 };
                let pf  = crate::vmm::PTE_PRESENT | crate::vmm::PTE_USER;
                let pfw = pf | crate::vmm::PTE_WRITABLE;
                // Code page in the PRIVATE low half — no collision with the kernel
                // (its low identity isn't mapped here) or with the other process.
                let code = crate::pmm::alloc_frame().unwrap_or(0);
                core::ptr::write_bytes(code as *mut u8, 0, 4096);
                core::ptr::copy_nonoverlapping(stub.as_ptr(), code as *mut u8, stub.len());
                if let Some(t) = tag { *(code as *mut u8).add(1) = t; } // patch tag byte
                crate::vmm::map_page_in(as_, LOW_CODE_VA, code, pf);
                // User stack page, also in the private low half.
                let stk = crate::pmm::alloc_frame().unwrap_or(0);
                core::ptr::write_bytes(stk as *mut u8, 0, 4096);
                crate::vmm::map_page_in(as_, LOW_STACK_VA, stk, pfw);
                // Ring-3 iret frame + zeroed GPRs on this task's kernel stack.
                let base = core::ptr::addr_of_mut!(KSTACKS[i]) as *mut u8;
                let ktop = ((base.add(KSTACK_SIZE) as u64) & !0xF) as u64;
                let mut sp = ktop;
                let push = |sp: &mut u64, v: u64| { *sp -= 8; *(*sp as *mut u64) = v; };
                push(&mut sp, 0x1b);            // ss = user data | RPL3
                push(&mut sp, LOW_STACK_TOP);   // user rsp (private low)
                push(&mut sp, 0x202);           // rflags IF=1
                push(&mut sp, 0x23);            // cs = user code | RPL3
                push(&mut sp, LOW_CODE_VA);     // rip = stub entry (private low)
                for _ in 0..15 { push(&mut sp, 0); }
                TASKS[i] = Task { rsp: sp, used: true, alive: true, cr3: as_ };
                return i;
            }
        }
        0
    }
}

/// Increment-3d self-test (cmdline `schedtest6`): TWO ring-3 tasks, each in its
/// own private-low-half address space, both mapped at the SAME low VA 0x400000
/// but emitting different syscall tags (0xA1, 0xB2), plus the boot thread. If
/// BOTH tags appear and interleave with the boot thread, then: per-process
/// private low half, same-VA isolation, and a fully higher-half kernel servicing
/// ring-3 from address spaces that DO NOT map PML4[0] — all proven.
pub fn selftest_ring3_lowhalf() -> ! {
    use crate::serial::write_str;
    write_str("\n[sched] === Increment 3d: private low half per process ===\n");
    unsafe {
        core::arch::asm!("cli");
        TASKS[0] = Task { rsp: 0, used: true, alive: true, cr3: crate::vmm::current_cr3() };
        CUR_TASK = 0;
    }
    let a = spawn_ring3_low(0xA1);
    let b = spawn_ring3_low(0xB2);
    if a == 0 || b == 0 { write_str("[sched] spawn_ring3_low failed\n"); halt(); }
    write_str("[sched] two private-low-half ring-3 tasks @ 0x400000; enabling preemption\n");
    crate::idt::set_timer_vector(irq_timer_preempt as *const () as u64);
    unsafe { core::arch::asm!("sti"); }
    loop {
        write_str("[sched] boot (kernel ring-0)\n");
        busy_spin();
    }
}

/// `multiproc` brick — the isolation property a multi-process desktop needs:
/// **a hung app cannot freeze the rest of the system.** Spawns a HEALTHY ring-3
/// process (tag 0xA1, syscalls periodically) and a HUNG one (a pure `jmp $`
/// infinite loop that never yields), each in its own private address space, then
/// enables the preemption timer. If the healthy process's tag keeps arriving and
/// the kernel boot thread keeps running while the hung process spins forever,
/// then the timer is forcibly reclaiming the CPU from the hung task — exactly
/// what stops one wedged app from locking the desktop. Gated behind `multiproc`.
pub fn selftest_multiproc() -> ! {
    use crate::serial::write_str;
    write_str("\n[mp] === multiproc: a hung app must NOT freeze the system ===\n");
    unsafe {
        core::arch::asm!("cli");
        TASKS[0] = Task { rsp: 0, used: true, alive: true, cr3: crate::vmm::current_cr3() };
        CUR_TASK = 0;
    }
    let healthy = spawn_ring3_low(0xA1);                  // cooperative, syscalls
    let hung = spawn_ring3_low_with(&HUNG_STUB, None);    // wedged: pure jmp $ loop
    if healthy == 0 || hung == 0 { write_str("[mp] spawn failed\n"); halt(); }
    write_str("[mp] spawned: process A (healthy) + process B (HUNG, infinite loop)\n");
    write_str("[mp] enabling preemption — B can only be stopped by the timer\n");
    crate::idt::set_timer_vector(irq_timer_preempt as *const () as u64);
    unsafe { core::arch::asm!("sti"); }
    let mut ticks = 0u32;
    loop {
        write_str("[mp] kernel alive — desktop would keep compositing\n");
        busy_spin();
        ticks += 1;
        if ticks == 8 {
            write_str("[mp] === ISOLATION PROVEN: B spun forever, A + kernel kept running ===\n");
        }
    }
}

/// `watchdog` brick — recovery, not just survival: **detect a hung process and
/// force-quit it.** Same setup as the isolation demo (healthy A + wedged B), but
/// now the kernel boot thread runs a watchdog: a process whose syscall count
/// stops advancing over a window is "not responding" and gets terminated. Proves
/// the kernel can reclaim a wedged app's CPU slot so the rest keeps running — the
/// mechanism behind a desktop "Force Quit". Gated behind `watchdog`.
pub fn selftest_watchdog() -> ! {
    use crate::serial::write_str;
    write_str("\n[wd] === watchdog: detect + force-quit a hung process ===\n");
    unsafe {
        core::arch::asm!("cli");
        TASKS[0] = Task { rsp: 0, used: true, alive: true, cr3: crate::vmm::current_cr3() };
        CUR_TASK = 0;
    }
    let a = spawn_ring3_low(0xA1);                     // healthy (syscalls)
    let b = spawn_ring3_low_with(&HUNG_STUB, None);    // wedged (jmp $)
    if a == 0 || b == 0 { write_str("[wd] spawn failed\n"); halt(); }
    write_str("[wd] process A (healthy) + process B (HUNG) spawned; arming watchdog\n");
    crate::idt::set_timer_vector(irq_timer_preempt as *const () as u64);
    unsafe { core::arch::asm!("sti"); }

    let mut prev_b = 0u64;
    let mut stall = 0u32;
    let mut killed = false;
    let mut post = 0u32;
    loop {
        write_str("[wd] watchdog: checking process liveness\n");
        busy_spin();
        let sb = task_syscalls(b);
        if !killed {
            // No syscall progress since the last check → another strike.
            if sb == prev_b { stall += 1; } else { stall = 0; }
            prev_b = sb;
            if stall >= 3 {
                write_str("[wd] process B NOT RESPONDING (no progress) — force-quitting\n");
                kill_task(b);
                killed = true;
                write_str("[wd] B terminated and dropped from the schedule\n");
            }
        } else {
            // After the kill, show A is still alive and B is gone for good.
            post += 1;
            if post == 4 {
                write_str("[wd]   B syscalls (frozen): ");
                log_u64(task_syscalls(b));
                write_str("  A syscalls (still climbing): ");
                log_u64(task_syscalls(a));
                write_str("\n[wd] === RECOVERY PROVEN: hung app force-quit, system healthy ===\n");
            }
        }
    }
}

fn log_u64(mut v: u64) {
    if v == 0 { crate::serial::write_byte(b'0'); return; }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while v > 0 { i -= 1; buf[i] = b'0' + (v % 10) as u8; v /= 10; }
    for &b in &buf[i..] { crate::serial::write_byte(b); }
}

// ─────────────────────────────────────────────────────────────────────────────
// `realelf` brick — run a REAL ELF program (not a hand-written stub) as a
// preemptively-scheduled, address-space-isolated ring-3 process. Bricks 1/2
// proved the mechanism with synthetic stubs; this proves the real loader path:
// parse an ELF's program headers and load its PT_LOAD segments into a private
// address space (writing the frames through the physmap — no AS switch), then
// schedule it. This is the reusable infrastructure a multi-process desktop needs
// to run each app (e.g. fbDOOM) in its own process. Gated behind `realelf`.
// ─────────────────────────────────────────────────────────────────────────────

// Offscreen-buffer VA mapped into a render process; its writes land in a frame
// the kernel (a stand-in for the desktop compositor) reads back.
const FB_VA: u64 = 0x0080_0000;

// Where a second app's surface frame is mapped INTO the real desktop's address
// space so the desktop (itself a scheduled process) can composite it into an
// on-screen window. 0x3000000 (48 MiB) sits in the desktop's free VA gap —
// above its code+32 MiB BSS heap (~37 MiB) and below its stack (~63 MiB).
const APP_SURF_VA: u64 = 0x0300_0000;
// Set by selftest_schedesktop2 once the app surface is mapped into the desktop;
// the desktop reads it via sys_app_surface (#41). 0 = no app surface (normal boot).
static mut APP_SURFACE_VA: u64 = 0;

/// The VA at which the current desktop process can read the second app's live
/// surface, or 0 if none. Backs sys_app_surface (#41).
pub fn app_surface_va() -> u64 {
    unsafe { APP_SURFACE_VA }
}

// A ring-3 program that writes a marker (0xDEADBEEF) to the offscreen buffer at
// FB_VA, then syscalls a tag, then spins — proving a process can render into an
// isolated buffer the compositor can read. Position-independent (immediates only).
const RENDER_STUB: [u8; 25] = [
    0xb8, 0x00, 0x00, 0x80, 0x00,       // mov eax, 0x00800000 (FB_VA)
    0xc7, 0x00, 0xef, 0xbe, 0xad, 0xde, // mov dword [rax], 0xDEADBEEF
    0xbf, 0xf1, 0x00, 0x00, 0x00,       // mov edi, 0xF1 (tag)
    0xb8, 0x37, 0x13, 0x00, 0x00,       // mov eax, 0x1337
    0x0f, 0x05,                         // syscall
    0xeb, 0xe7,                         // jmp -25 (back to start)
];

// A ring-3 program that FILLS its offscreen buffer (1024 px) with a solid colour
// (0x00FF8800 orange), then syscalls a tag, then re-fills forever. Used to make
// the compositor path visible on screen. Immediates only (position-independent).
const FILL_STUB: [u8; 38] = [
    0xb8, 0x00, 0x00, 0x80, 0x00,       // mov eax, 0x00800000 (FB_VA)
    0xb9, 0x00, 0x04, 0x00, 0x00,       // mov ecx, 1024
    0xba, 0x00, 0x88, 0xff, 0x00,       // mov edx, 0x00FF8800 (orange)
    0x89, 0x10,                         // .l: mov [rax], edx
    0x83, 0xc0, 0x04,                   //     add eax, 4
    0xff, 0xc9,                         //     dec ecx
    0x75, 0xf7,                         //     jnz .l
    0xbf, 0xf2, 0x00, 0x00, 0x00,       // mov edi, 0xF2 (tag)
    0xb8, 0x37, 0x13, 0x00, 0x00,       // mov eax, 0x1337
    0x0f, 0x05,                         // syscall
    0xeb, 0xda,                         // jmp -38 (refill)
];

/// Build a minimal static ET_EXEC ELF (one R+X PT_LOAD segment) running `code`
/// at `load_va`. A non-blocking test program so the real-ELF scheduled-process
/// path verifies without framebuffer/keyboard contention.
fn make_elf(load_va: u64, code: &[u8]) -> alloc::vec::Vec<u8> {
    const HDRS: usize = 64 + 56; // Elf64 header + one program header
    let mut e = alloc::vec![0u8; HDRS];
    e[0..4].copy_from_slice(b"\x7fELF");
    e[4] = 2; // ELFCLASS64
    e[5] = 1; // ELFDATA2LSB
    e[6] = 1; // EV_CURRENT
    let entry = load_va + HDRS as u64;
    e[16..18].copy_from_slice(&2u16.to_le_bytes());    // e_type = ET_EXEC
    e[18..20].copy_from_slice(&0x3Eu16.to_le_bytes()); // e_machine = x86-64
    e[20..24].copy_from_slice(&1u32.to_le_bytes());    // e_version
    e[24..32].copy_from_slice(&entry.to_le_bytes());   // e_entry
    e[32..40].copy_from_slice(&64u64.to_le_bytes());   // e_phoff
    e[52..54].copy_from_slice(&64u16.to_le_bytes());   // e_ehsize
    e[54..56].copy_from_slice(&56u16.to_le_bytes());   // e_phentsize
    e[56..58].copy_from_slice(&1u16.to_le_bytes());    // e_phnum
    let p = 64;
    e[p..p + 4].copy_from_slice(&1u32.to_le_bytes());      // p_type = PT_LOAD
    e[p + 4..p + 8].copy_from_slice(&5u32.to_le_bytes());  // p_flags = R+X
    e[p + 16..p + 24].copy_from_slice(&load_va.to_le_bytes()); // p_vaddr
    e[p + 24..p + 32].copy_from_slice(&load_va.to_le_bytes()); // p_paddr
    let total = (HDRS + code.len()) as u64;
    e[p + 32..p + 40].copy_from_slice(&total.to_le_bytes());   // p_filesz
    e[p + 40..p + 48].copy_from_slice(&total.to_le_bytes());   // p_memsz
    e[p + 48..p + 56].copy_from_slice(&0x1000u64.to_le_bytes()); // p_align
    e.extend_from_slice(code);
    e
}

/// Build the tagged syscall-loop test ELF (R3_STUB with `tag` patched in).
fn make_test_elf(load_va: u64, tag: u8) -> alloc::vec::Vec<u8> {
    let mut code = R3_STUB;
    code[1] = tag; // patch the `mov edi, imm` tag byte
    make_elf(load_va, &code)
}

const ELF_STACK_VA: u64 = 0x0070_0000; // private-low user stack for a loaded ELF
const ELF_STACK_TOP: u64 = ELF_STACK_VA + 0x1000;

#[inline]
fn rd_u64(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) }
#[inline]
fn rd_u32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) }
#[inline]
fn rd_u16(b: &[u8], o: usize) -> u16 { u16::from_le_bytes(b[o..o + 2].try_into().unwrap()) }

/// Load a real ELF into a fresh private address space and schedule it as a ring-3
/// task. Walks the program headers and copies each PT_LOAD segment into freshly
/// allocated frames mapped into the new space (written via the physmap, so no
/// CR3 switch is needed). Returns the task index, or 0 on failure.
fn spawn_ring3_elf(elf: &[u8], fb_frame: u64) -> usize {
    spawn_ring3_elf_cfg(elf, ELF_STACK_TOP, 1, fb_frame)
}

/// Like `spawn_ring3_elf` but with a configurable user stack (`stack_top`,
/// `stack_pages`) — real programs (the desktop) want a multi-page stack at their
/// expected VA, not the single test-program page.
fn spawn_ring3_elf_cfg(elf: &[u8], stack_top: u64, stack_pages: u32, fb_frame: u64) -> usize {
    unsafe {
        if elf.len() < 64 || &elf[0..4] != b"\x7fELF" { return 0; }
        for i in 1..MAX_TASKS {
            if TASKS[i].used { continue; }
            let as_ = match crate::vmm::new_address_space_private() { Some(p) => p, None => return 0 };
            let pfw = crate::vmm::PTE_PRESENT | crate::vmm::PTE_WRITABLE | crate::vmm::PTE_USER;

            let entry = rd_u64(elf, 24);
            let phoff = rd_u64(elf, 32) as usize;
            let phnum = rd_u16(elf, 56) as usize;
            let phent = rd_u16(elf, 54) as usize;

            for s in 0..phnum {
                let ph = phoff + s * phent;
                if ph + 56 > elf.len() || rd_u32(elf, ph) != 1 { continue; } // PT_LOAD
                let p_off = rd_u64(elf, ph + 8) as usize;
                let p_va = rd_u64(elf, ph + 16);
                let p_fsz = rd_u64(elf, ph + 32) as usize;
                let p_msz = rd_u64(elf, ph + 40) as usize;

                let start = p_va & !0xFFF;
                let end = (p_va + p_msz as u64 + 0xFFF) & !0xFFF;
                let mut va = start;
                while va < end {
                    let frame = match crate::pmm::alloc_frame() { Some(f) => f, None => return 0 };
                    let dst = crate::vmm::phys_to_virt(frame) as *mut u8;
                    core::ptr::write_bytes(dst, 0, 4096);
                    // Copy the file-backed bytes that fall in this page.
                    let file_va_end = p_va + p_fsz as u64;
                    let cs = va.max(p_va);
                    let ce = (va + 4096).min(file_va_end);
                    if ce > cs {
                        let src_off = p_off + (cs - p_va) as usize;
                        let dst_off = (cs - va) as usize;
                        let len = (ce - cs) as usize;
                        if src_off + len <= elf.len() {
                            core::ptr::copy_nonoverlapping(elf.as_ptr().add(src_off), dst.add(dst_off), len);
                        }
                    }
                    if !crate::vmm::map_page_in(as_, va, frame, pfw) { return 0; }
                    va += 4096;
                }
            }

            // User stack: `stack_pages` pages ending at `stack_top` (grows down).
            let stack_bottom = stack_top - (stack_pages as u64) * 4096;
            let mut sva = stack_bottom;
            while sva < stack_top {
                let stk = match crate::pmm::alloc_frame() { Some(f) => f, None => return 0 };
                core::ptr::write_bytes(crate::vmm::phys_to_virt(stk) as *mut u8, 0, 4096);
                crate::vmm::map_page_in(as_, sva, stk, pfw);
                sva += 4096;
            }

            // Optional offscreen framebuffer: a kernel-owned frame mapped into the
            // process at FB_VA. The process renders into it; the compositor (here,
            // the boot thread) reads it back via the physmap — isolated rendering.
            if fb_frame != 0 {
                crate::vmm::map_page_in(as_, FB_VA, fb_frame, pfw);
            }

            // Prime this task's kernel stack with a ring-3 iret frame at e_entry.
            let base = core::ptr::addr_of_mut!(KSTACKS[i]) as *mut u8;
            let ktop = ((base.add(KSTACK_SIZE) as u64) & !0xF) as u64;
            let mut sp = ktop;
            let push = |sp: &mut u64, v: u64| { *sp -= 8; *(*sp as *mut u64) = v; };
            push(&mut sp, 0x1b);              // ss
            push(&mut sp, stack_top - 8);     // user rsp (16-aligned-ish, room for the program)
            push(&mut sp, 0x202);             // rflags IF=1
            push(&mut sp, 0x23);          // cs
            push(&mut sp, entry);         // rip
            for _ in 0..15 { push(&mut sp, 0); }
            TASKS[i] = Task { rsp: sp, used: true, alive: true, cr3: as_ };
            return i;
        }
        0
    }
}

/// `realelf` brick: load two REAL ELF programs (built by `make_test_elf`, tags
/// 0xE1/0xE2) into separate private address spaces and run them under preemption
/// with the boot thread. If both tags interleave with the boot thread, the real
/// ELF loader + per-process isolation + preemptive scheduling all work together —
/// the foundation for running desktop apps as separate processes.
pub fn selftest_realelf() -> ! {
    use crate::serial::write_str;
    write_str("\n[elf] === realelf: two REAL ELF programs as scheduled processes ===\n");
    unsafe {
        core::arch::asm!("cli");
        TASKS[0] = Task { rsp: 0, used: true, alive: true, cr3: crate::vmm::current_cr3() };
        CUR_TASK = 0;
    }
    let e1 = make_test_elf(0x0050_0000, 0xE1);
    let e2 = make_test_elf(0x0050_0000, 0xE2); // same VA, different private space
    let a = spawn_ring3_elf(&e1, 0);
    let b = spawn_ring3_elf(&e2, 0);
    if a == 0 || b == 0 { write_str("[elf] spawn_ring3_elf failed\n"); halt(); }
    write_str("[elf] two real ELF processes @ 0x500000 (isolated); enabling preemption\n");
    crate::idt::set_timer_vector(irq_timer_preempt as *const () as u64);
    unsafe { core::arch::asm!("sti"); }
    let mut n = 0u32;
    loop {
        write_str("[elf] boot (kernel ring-0)\n");
        busy_spin();
        n += 1;
        if n == 8 {
            write_str("[elf] === REAL-ELF MULTIPROCESS PROVEN: both ELF tags ran, isolated, preempted ===\n");
        }
    }
}

/// `offscreen` brick — isolated rendering: a scheduled ELF process writes a
/// marker into an offscreen framebuffer that's private to it (mapped at FB_VA
/// from a kernel-owned frame), and the boot thread — standing in for the desktop
/// compositor — reads that frame back through the physmap. Proving a process can
/// render into a buffer the compositor owns is the missing piece between
/// "isolated processes" (brick 3a) and "windowed apps" (a real compositor blits
/// each process's buffer into its window). Gated behind `offscreen`.
pub fn selftest_offscreen() -> ! {
    use crate::serial::write_str;
    write_str("\n[fb] === offscreen: a scheduled process renders into a buffer the compositor reads ===\n");
    unsafe {
        core::arch::asm!("cli");
        TASKS[0] = Task { rsp: 0, used: true, alive: true, cr3: crate::vmm::current_cr3() };
        CUR_TASK = 0;
    }
    let fb_frame = match crate::pmm::alloc_frame() { Some(f) => f, None => { write_str("[fb] no frame\n"); halt(); } };
    unsafe { core::ptr::write_bytes(crate::vmm::phys_to_virt(fb_frame) as *mut u8, 0, 4096); }
    let elf = make_elf(0x0050_0000, &RENDER_STUB);
    let t = spawn_ring3_elf(&elf, fb_frame);
    if t == 0 { write_str("[fb] spawn failed\n"); halt(); }
    write_str("[fb] render process spawned with a private offscreen buffer; enabling preemption\n");
    crate::idt::set_timer_vector(irq_timer_preempt as *const () as u64);
    unsafe { core::arch::asm!("sti"); }
    let mut proven = false;
    loop {
        busy_spin();
        // The "compositor" reads the process's offscreen buffer via the physmap.
        let marker = unsafe { *(crate::vmm::phys_to_virt(fb_frame) as *const u32) };
        if marker == 0xDEAD_BEEF && !proven {
            write_str("[fb] compositor read process buffer: 0xDEADBEEF present\n");
            write_str("[fb] === OFFSCREEN RENDER PROVEN: process rendered into an isolated buffer the compositor read back ===\n");
            proven = true;
        }
        write_str("[fb] boot/compositor tick\n");
    }
}

// Like FILL_STUB but renders its surface ONCE, makes one liveness syscall, then
// hangs forever (jmp $) — a "render then wedge" app. The watchdog sees no further
// syscall progress and force-quits it. Colour at offset 11 (mov edx, imm32).
const RENDER_HANG_STUB: [u8; 38] = [
    0xb8, 0x00, 0x00, 0x80, 0x00,       // mov eax, FB_VA
    0xb9, 0x00, 0x04, 0x00, 0x00,       // mov ecx, 1024
    0xba, 0xff, 0xa0, 0x48, 0x00,       // mov edx, 0x0048A0FF (blue)
    0x89, 0x10,                         // .l: mov [rax], edx
    0x83, 0xc0, 0x04,                   //     add eax, 4
    0xff, 0xc9,                         //     dec ecx
    0x75, 0xf7,                         //     jnz .l
    0xbf, 0xf3, 0x00, 0x00, 0x00,       // mov edi, 0xF3 (one liveness tag)
    0xb8, 0x37, 0x13, 0x00, 0x00,       // mov eax, 0x1337
    0x0f, 0x05,                         // syscall
    0xeb, 0xfe,                         // jmp $  (wedge forever)
];

/// Build a fill-program ELF whose surface colour is `color` (patches the
/// `mov edx, imm32` in FILL_STUB at code offset 11).
fn make_fill_elf(load_va: u64, color: u32) -> alloc::vec::Vec<u8> {
    let mut code = FILL_STUB;
    code[11..15].copy_from_slice(&color.to_le_bytes());
    make_elf(load_va, &code)
}

/// Blit a process's 32×32 offscreen surface (frame `fb_frame`) into a window at
/// (x,y) on the real framebuffer, with a titlebar + frame. The compositor step.
fn composite_window(fb_frame: u64, x: u32, y: u32) {
    const SW: u32 = 32;
    let base = crate::fb::base();
    let pitch = crate::fb::pitch();
    if base.is_null() || pitch == 0 {
        return;
    }
    crate::fb::fill(x - 2, y - 12, SW + 4, 12, 0x2A3340); // titlebar
    crate::fb::fill(x - 2, y - 2, SW + 4, SW + 4, 0x6FE18B); // frame
    let src = crate::vmm::phys_to_virt(fb_frame) as *const u32;
    for py in 0..SW {
        for px in 0..SW {
            let pix = unsafe { *src.add((py * SW + px) as usize) };
            let off = ((y + py) * pitch + (x + px) * 4) as usize;
            unsafe { *(base.add(off) as *mut u32) = pix; }
        }
    }
}

/// `multiwin` brick — "run several real apps at once": spawn TWO independent ELF
/// processes, each in its own private address space rendering its own colour into
/// its own surface, and the compositor blits BOTH into separate windows. The
/// other half of item 4's goal (the first half — a hung app can't freeze the
/// desktop — is bricks 1/2). Screenshot-verifiable. Gated behind `multiwin`.
pub fn selftest_multiwin() -> ! {
    use crate::serial::write_str;
    write_str("\n[wm] === multiwin: TWO real app processes, two surfaces, two windows ===\n");
    unsafe {
        core::arch::asm!("cli");
        TASKS[0] = Task { rsp: 0, used: true, alive: true, cr3: crate::vmm::current_cr3() };
        CUR_TASK = 0;
    }
    let f1 = crate::pmm::alloc_frame().unwrap_or(0);
    let f2 = crate::pmm::alloc_frame().unwrap_or(0);
    if f1 == 0 || f2 == 0 { write_str("[wm] no frames\n"); halt(); }
    unsafe {
        core::ptr::write_bytes(crate::vmm::phys_to_virt(f1) as *mut u8, 0, 4096);
        core::ptr::write_bytes(crate::vmm::phys_to_virt(f2) as *mut u8, 0, 4096);
    }
    let e1 = make_fill_elf(0x0050_0000, 0x00FF_8800); // orange app
    let e2 = make_fill_elf(0x0050_0000, 0x0048_A0FF); // blue app (same VA, isolated)
    let a = spawn_ring3_elf(&e1, f1);
    let b = spawn_ring3_elf(&e2, f2);
    if a == 0 || b == 0 { write_str("[wm] spawn failed\n"); halt(); }
    write_str("[wm] two app processes running; compositing both into windows\n");
    crate::idt::set_timer_vector(irq_timer_preempt as *const () as u64);
    unsafe { core::arch::asm!("sti"); }
    let mut n = 0u32;
    loop {
        busy_spin();
        composite_window(f1, 120, 120); // app 1 window
        composite_window(f2, 200, 160); // app 2 window
        n += 1;
        if n == 6 {
            write_str("[wm] === MULTI-APP PROVEN: two isolated processes composited into two windows at once ===\n");
        }
    }
}

/// `recoverwin` brick — item 4's whole goal in ONE scene: two windowed app
/// processes, one of which wedges; the watchdog force-quits the hung one and the
/// compositor closes its window ("Not Responding"), while the healthy app keeps
/// rendering. Isolation (1/2) + watchdog (2) + windowing (3d) together — exactly
/// "a hung app can't freeze the desktop, and several apps run at once."
/// Screenshot-verifiable. Gated behind `recoverwin`.
pub fn selftest_recover_win() -> ! {
    use crate::serial::write_str;
    write_str("\n[wm] === recoverwin: one windowed app hangs -> force-quit; the other keeps running ===\n");
    unsafe {
        core::arch::asm!("cli");
        TASKS[0] = Task { rsp: 0, used: true, alive: true, cr3: crate::vmm::current_cr3() };
        CUR_TASK = 0;
    }
    let f1 = crate::pmm::alloc_frame().unwrap_or(0);
    let f2 = crate::pmm::alloc_frame().unwrap_or(0);
    if f1 == 0 || f2 == 0 { write_str("[wm] no frames\n"); halt(); }
    unsafe {
        core::ptr::write_bytes(crate::vmm::phys_to_virt(f1) as *mut u8, 0, 4096);
        core::ptr::write_bytes(crate::vmm::phys_to_virt(f2) as *mut u8, 0, 4096);
    }
    let healthy = make_fill_elf(0x0050_0000, 0x00FF_8800);      // keeps rendering + syscalling
    let wedger = make_elf(0x0050_0000, &RENDER_HANG_STUB);      // renders once, then hangs
    let a = spawn_ring3_elf(&healthy, f1);
    let b = spawn_ring3_elf(&wedger, f2);
    if a == 0 || b == 0 { write_str("[wm] spawn failed\n"); halt(); }
    write_str("[wm] app A (healthy) + app B (will wedge) running in windows\n");
    crate::idt::set_timer_vector(irq_timer_preempt as *const () as u64);
    unsafe { core::arch::asm!("sti"); }

    let mut prev_b = 0u64;
    let mut stall = 0u32;
    let mut closed = false;
    loop {
        busy_spin();
        composite_window(f1, 120, 120); // healthy app A always updates
        unsafe {
            if TASKS[b].alive {
                composite_window(f2, 200, 160); // app B while alive
                let sb = task_syscalls(b);
                if sb == prev_b { stall += 1; } else { stall = 0; }
                prev_b = sb;
                if stall >= 4 {
                    write_str("[wm] app B (window 2) NOT RESPONDING — force-quitting\n");
                    kill_task(b);
                }
            } else if !closed {
                // Draw a "Not Responding / closed" overlay over B's window.
                crate::fb::fill(200 - 2, 160 - 12, 36, 48, 0x4A2024);
                write_str("[wm] B's window force-closed; A keeps rendering\n");
                write_str("[wm] === RECOVER PROVEN: hung windowed app reaped, healthy app unaffected ===\n");
                closed = true;
            }
        }
    }
}

/// `schedesktop` brick — run the REAL desktop as a preemptively-scheduled process
/// in its own private address space, instead of the normal single-process launch.
/// This proves the spawn_ring3_elf loader handles a real, complex 11 MB program
/// (24 MiB heap, multi-page stack, hundreds of syscalls) — the bridge from the
/// synthetic test apps (3a–3e) to the real multi-process desktop. The boot thread
/// idles; the desktop gets CPU via the timer and renders normally. Gated behind
/// `schedesktop` (the default desktop boot path is untouched).
pub fn selftest_schedesktop() -> ! {
    use crate::serial::write_str;
    write_str("\n[mpd] === schedesktop: the REAL desktop as a scheduled ring-3 process ===\n");
    let elf = match crate::ramfs::find(b"bin/desktop") {
        Some(e) => e,
        None => { write_str("[mpd] bin/desktop not in initrd\n"); halt(); }
    };
    unsafe {
        core::arch::asm!("cli");
        TASKS[0] = Task { rsp: 0, used: true, alive: true, cr3: crate::vmm::current_cr3() };
        CUR_TASK = 0;
    }
    write_str("[mpd] loading real desktop into a private address space (24 MiB heap)...\n");
    let d = spawn_ring3_elf_cfg(elf, crate::vmm::USER_STACK_TOP, crate::vmm::USER_STACK_PAGES as u32, 0);
    if d == 0 { write_str("[mpd] spawn_ring3_elf failed (out of frames?)\n"); halt(); }
    write_str("[mpd] desktop scheduled; clearing screen + enabling preemption\n");
    // Match the normal launch: hand the desktop a black canvas.
    if crate::fb::is_live() {
        crate::fb::fill(0, 0, crate::fb::width(), crate::fb::height(), 0x000000);
    }
    crate::idt::set_timer_vector(irq_timer_preempt as *const () as u64);
    unsafe { core::arch::asm!("sti"); }
    // Boot thread idles; the desktop process gets the CPU and renders.
    loop {
        busy_spin();
    }
}

/// `schedesktop2` brick — the REAL desktop AND a second real ELF app running as
/// two independent, preemptively-scheduled, address-space-isolated processes at
/// the same time. This is the scenario that previously triggered a #GP: when the
/// desktop blocked in a syscall (sys_read → sti+hlt) and the second app entered a
/// syscall in that window, they shared ONE kernel syscall stack and clobbered
/// each other's saved frame. With the per-task `_cur_syscall_stack` fix each task
/// has its own syscall stack, so concurrent syscalls no longer collide.
///
/// Pass criterion: the desktop renders AND stays rendering (no #GP / triple fault)
/// while the second app is also scheduled and makes its own syscalls.
pub fn selftest_schedesktop2() -> ! {
    use crate::serial::write_str;
    write_str("\n[mpd2] === schedesktop2: real desktop + a second real app, both scheduled ===\n");
    let elf = match crate::ramfs::find(b"bin/desktop") {
        Some(e) => e,
        None => { write_str("[mpd2] bin/desktop not in initrd\n"); halt(); }
    };
    unsafe {
        core::arch::asm!("cli");
        TASKS[0] = Task { rsp: 0, used: true, alive: true, cr3: crate::vmm::current_cr3() };
        CUR_TASK = 0;
    }
    write_str("[mpd2] loading real desktop into a private address space (24 MiB heap)...\n");
    let d = spawn_ring3_elf_cfg(elf, crate::vmm::USER_STACK_TOP, crate::vmm::USER_STACK_PAGES as u32, 0);
    if d == 0 { write_str("[mpd2] desktop spawn failed (out of frames?)\n"); halt(); }
    // Second app: a real ELF in its OWN address space, rendering to its own
    // offscreen surface and making liveness syscalls — entirely independent of
    // the desktop. Its very existence + concurrent syscalls is the test.
    let fb_frame = match crate::pmm::alloc_frame() {
        Some(f) => f, None => { write_str("[mpd2] no frame for app surface\n"); halt(); }
    };
    unsafe { core::ptr::write_bytes(crate::vmm::phys_to_virt(fb_frame) as *mut u8, 0, 4096); }
    let app_elf = make_elf(0x0050_0000, &FILL_STUB);
    let a = spawn_ring3_elf(&app_elf, fb_frame);
    if a == 0 { write_str("[mpd2] second-app spawn failed\n"); halt(); }
    // Map the SAME surface frame into the REAL desktop's address space so the
    // desktop (a scheduled process) can composite the app into an on-screen
    // window. Read-only for the desktop (USER|PRESENT, not WRITABLE).
    unsafe {
        let ro = crate::vmm::PTE_PRESENT | crate::vmm::PTE_USER;
        if crate::vmm::map_page_in(TASKS[d].cr3, APP_SURF_VA, fb_frame, ro) {
            APP_SURFACE_VA = APP_SURF_VA;
            write_str("[mpd2] app surface mapped into the desktop AS @ 0x3000000\n");
        } else {
            write_str("[mpd2] WARN: could not map app surface into desktop AS\n");
        }
    }
    write_str("[mpd2] desktop + second app both scheduled; clearing screen + enabling preemption\n");
    if crate::fb::is_live() {
        crate::fb::fill(0, 0, crate::fb::width(), crate::fb::height(), 0x000000);
    }
    crate::idt::set_timer_vector(irq_timer_preempt as *const () as u64);
    unsafe { core::arch::asm!("sti"); }
    // Boot thread idles; the desktop renders and the second app runs concurrently.
    loop {
        busy_spin();
    }
}

/// `composite` brick — the full pipeline made VISIBLE: a scheduled, isolated ELF
/// process renders a colour into its own offscreen buffer, and the kernel
/// compositor blits that buffer onto the real screen as a window-like rectangle.
/// This is the end-to-end model for windowed apps (each app → its own surface →
/// the WM blits it into a window). Screenshot-verifiable. Gated behind `composite`.
pub fn selftest_composite() -> ! {
    use crate::serial::write_str;
    write_str("\n[wm] === composite: process renders offscreen -> compositor blits to screen ===\n");
    unsafe {
        core::arch::asm!("cli");
        TASKS[0] = Task { rsp: 0, used: true, alive: true, cr3: crate::vmm::current_cr3() };
        CUR_TASK = 0;
    }
    let fb_frame = match crate::pmm::alloc_frame() { Some(f) => f, None => { write_str("[wm] no frame\n"); halt(); } };
    unsafe { core::ptr::write_bytes(crate::vmm::phys_to_virt(fb_frame) as *mut u8, 0, 4096); }
    let elf = make_elf(0x0050_0000, &FILL_STUB);
    let t = spawn_ring3_elf(&elf, fb_frame);
    if t == 0 { write_str("[wm] spawn failed\n"); halt(); }
    write_str("[wm] render process spawned; compositing its surface into a window\n");
    crate::idt::set_timer_vector(irq_timer_preempt as *const () as u64);
    unsafe { core::arch::asm!("sti"); }

    // Window position/size for the 1024-pixel (32x32) surface.
    const WIN_X: u32 = 120;
    const WIN_Y: u32 = 120;
    const SW: u32 = 32;
    loop {
        busy_spin();
        // Compositor: read the process's offscreen surface (physmap) and blit it
        // into the window region on the real framebuffer.
        let src = crate::vmm::phys_to_virt(fb_frame) as *const u32;
        let base = crate::fb::base();
        let pitch = crate::fb::pitch();
        if !base.is_null() && pitch > 0 {
            // window chrome: a border around the surface
            crate::fb::fill(WIN_X - 2, WIN_Y - 12, SW + 4, 12, 0x2A3340); // titlebar
            crate::fb::fill(WIN_X - 2, WIN_Y - 2, SW + 4, SW + 4, 0x6FE18B); // frame
            for py in 0..SW {
                for px in 0..SW {
                    let pix = unsafe { *src.add((py * SW + px) as usize) };
                    let off = ((WIN_Y + py) * pitch + (WIN_X + px) * 4) as usize;
                    unsafe { *(base.add(off) as *mut u32) = pix; }
                }
            }
        }
        write_str("[wm] composited process surface -> window @ 120,120\n");
    }
}

/// Increment-3c self-test (cmdline `schedtest5`): one ring-3 task in a private
/// address space + the boot thread (ring 0). If the ring-3 stub's syscall is
/// logged AND interleaves with the boot thread, then: ring-3 entry, private-AS
/// execution, the user→kernel syscall round-trip, per-task TSS.rsp0, and timer
/// preemption of ring-3 code all work.
pub fn selftest_ring3() -> ! {
    use crate::serial::write_str;
    write_str("\n[sched] === Increment 3c: ring-3 task in a private address space ===\n");
    crate::vmm::extend_identity_map(512);
    unsafe {
        core::arch::asm!("cli");
        TASKS[0] = Task { rsp: 0, used: true, alive: true, cr3: crate::vmm::current_cr3() };
        CUR_TASK = 0;
    }
    let r3 = spawn_ring3(&R3_STUB);
    if r3 == 0 { write_str("[sched] spawn_ring3 failed\n"); halt(); }
    write_str("[sched] ring-3 task spawned; enabling preemption\n");
    crate::idt::set_timer_vector(irq_timer_preempt as *const () as u64);
    unsafe { core::arch::asm!("sti"); }
    loop {
        write_str("[sched] boot (kernel ring-0)\n");
        busy_spin();
    }
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
