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
    cr3:   u64,  // address space (PML4 phys). 0 = don't switch (shared kernel AS).
}

static mut TASKS: [Task; MAX_TASKS] =
    [Task { rsp: 0, used: false, alive: false, cr3: 0 }; MAX_TASKS];
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
