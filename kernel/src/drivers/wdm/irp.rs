//! Windows IRP (I/O Request Packet) structures.
//! Used by WDM drivers to manage I/O requests.

#[repr(C)]
pub struct IoStackLocation {
    pub major_function: u8,
    pub minor_function: u8,
    pub flags: u8,
    pub control: u8,
    pub parameters: [u64; 4],
    pub device_object: u64,
    pub file_object: u64,
    pub completion_routine: u64,
}

#[repr(C)]
pub struct Irp {
    pub type_id: u16,
    pub size: u16,
    pub mdl_address: u64,
    pub flags: u32,
    pub associated_irp: u64,
    pub thread_list_entry: [u64; 2],
    pub io_status: [u64; 2], // Status, Information
    pub requestor_mode: u8,
    pub pending_returned: u8,
    pub stack_count: i8,
    pub current_location: i8,
    pub cancel: u8,
    pub cancel_irql: u8,
    pub apc_environment: u8,
    pub allocation_flags: u8,
    pub user_io_buffer: u64,
}
