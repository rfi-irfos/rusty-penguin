pub unsafe fn outl(port: u16, val: u32) {
    core::arch::asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack, preserves_flags));
}
pub unsafe fn inl(port: u16) -> u32 {
    let v: u32;
    core::arch::asm!("in eax, dx", in("dx") port, out("eax") v, options(nomem, nostack, preserves_flags));
    v
}
pub unsafe fn outw(port: u16, val: u16) {
    core::arch::asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack, preserves_flags));
}
pub unsafe fn inw(port: u16) -> u16 {
    let v: u16;
    core::arch::asm!("in ax, dx", in("dx") port, out("ax") v, options(nomem, nostack, preserves_flags));
    v
}
pub unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port, in("al") val,
        options(nomem, nostack, preserves_flags)
    );
}

pub unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port, out("al") val,
        options(nomem, nostack, preserves_flags)
    );
    val
}

pub unsafe fn io_wait() {
    outb(0x80, 0);
}
