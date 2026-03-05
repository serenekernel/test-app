#![no_std]
#![no_main]
extern crate alloc;

serenelib::serene_entry!(main);


use core::panic::PanicInfo;
use alloc::vec::Vec;
use serenelib::auxv::AuxVType;
use serenelib::debug_writer::{_print};
use serenelib::ipc::{Handle, IpcArray, IpcBytes, IpcPayloadBuilder, IpcPayloadReader};
use serenelib::syscalls::{sys_cap_port_grant, sys_endpoint_create, sys_endpoint_free_message, sys_endpoint_receive, sys_endpoint_send, sys_exit, sys_wait_for};
use serenelib::{print, println};
use x86_64::instructions::port::Port;

fn pci_read_config(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    let address: u32 = (1 << 31)
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xfc);

    unsafe {
        let mut port_address = Port::new(0xcf8);
        let mut port_data = Port::new(0xcfc);
        port_address.write(address);
        port_data.read()
    }
}

struct PciDevice {
    bus: u8,
    slot: u8,
    function: u8,
    vendor_id: u16,
    device_id: u16,
    header_type: u8,
    class_code: u8,
    subclass: u8,
    prog_if: u8,

    secondary_bus: Option<u8>,
}

impl PciDevice {
    pub fn new(bus: u8, slot: u8, function: u8) -> Self {
        let val = pci_read_config(bus, slot, function, 0);
        let vendor_id = (val & 0xffff) as u16;
        let device_id = ((val >> 16) & 0xffff) as u16;

        let val = pci_read_config(bus, slot, function, 8);
        let class_code = ((val >> 24) & 0xff) as u8;
        let subclass = ((val >> 16) & 0xff) as u8;
        let prog_if = ((val >> 8) & 0xff) as u8;

        let val = pci_read_config(bus, slot, function, 12);
        let header_type = ((val >> 16) & 0xff) as u8;
        let secondary_bus;
        if class_code == 0x6 && subclass == 0x4 {
            let val = pci_read_config(bus, slot, function, 24);
            secondary_bus = Some(((val >> 8) & 0xff) as u8);
        } else {
            secondary_bus = None;
        }

        PciDevice {
            bus,
            slot,
            function,
            vendor_id,
            device_id,
            header_type,
            class_code,
            subclass,
            prog_if,
            secondary_bus,
        }
    }

    pub fn exists(bus: u8, slot: u8, function: u8) -> bool {
        let val = pci_read_config(bus, slot, function, 0);
        let vendor_id = (val & 0xffff) as u16;
        vendor_id != 0xFFFF
    }
}

fn pci_check_function(bus: u8, device_num: u8, function: u8) {
    let device = PciDevice::new(bus, device_num, function);
    println!(
        "[pci] {}:{}.{} - {:04x}:{:04x} Class {:02x}:{:02x}",
        bus,
        device_num,
        function,
        device.vendor_id,
        device.device_id,
        device.class_code,
        device.subclass
    );
    if device.secondary_bus.is_some() {
        let secondary_bus = device.secondary_bus.unwrap();
        pci_scan_bus(secondary_bus);
    }
}

fn pci_check_device(bus: u8, device_num: u8) {
    if !PciDevice::exists(bus, device_num, 0) {
        return;
    }

    pci_check_function(bus, device_num, 0);
    let device = PciDevice::new(bus, device_num, 0);
    if (device.header_type & 0x80) != 0 {
        // It's a multi-function device, so check remaining functions
        for function in 1..8 {
            if PciDevice::exists(bus, device_num, function) {
                pci_check_function(bus, device_num, function);
            }
        }
    }
}

fn pci_scan_bus(bus: u8) {
    for device in 0..32 {
        pci_check_device(bus, device);
    }
}

fn pci_scan() {
    let device = PciDevice::new(0, 0, 0);
    if (device.header_type & 0x80) == 0 {
        pci_scan_bus(0);
    } else {
        for function in 0..8 {
            if !PciDevice::exists(0, 0, function) {
                break;
            }
            pci_scan_bus(function);
        }
    }
}

#[repr(C)]
struct IPC_Init_Discover {
    pub _type: u8,
    pub _pad: [u8; 7],
    pub name: IpcBytes,
}

#[repr(C)]
struct IPC_Init_DiscoverResponse {
    pub _type: u8,
    pub handle: Handle
}

#[repr(C)]
struct IPC_VFS_List_Dir_Request {
    pub _type: u8,
    pub _pad: [u8; 7],
    pub path: IpcBytes,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct IPC_VFS_List_Dir_Response_Entry {
    pub name: IpcBytes,
    pub flags: u8,
    pub _pad: [u8; 3],
}

#[repr(C)]
struct IPC_VFS_List_Dir_Response {
    pub _type: u8,
    pub _pad: [u8; 7],
    pub entries: IpcArray,
}


pub fn main(argv: Vec<&[u8]>, envp: Vec<&[u8]>, auxv: Vec<(u64, u64)>) -> i32 {
    println!("test_app: Starting up...");
    println!("test_app: argv length: {}, envp length: {}, auxv length: {}", argv.len(), envp.len(), auxv.len());
    for (i, arg) in argv.iter().enumerate() {
        if let Ok(arg_str) = core::str::from_utf8(arg) {
            println!("test_app: main arg {}: {}", i, arg_str);
        } else {
            println!("test_app: main arg {}: (invalid UTF-8)", i);
        }
    }

    for (i, env) in envp.iter().enumerate() {
        if let Ok(env_str) = core::str::from_utf8(env) {
            println!("test_app: main env {}: {}", i, env_str);
        } else {
            println!("test_app: main env {}: (invalid UTF-8)", i);
        }
    }

    let mut init_system_handle: Option<Handle> = None;

    for (ty, val) in auxv {
        println!("test_app: main auxv: type {}, val {:#x}", ty, val);
        if ty == AuxVType::AUXV_SERENE_INIT_HANDLE as u64 {
            init_system_handle = Some(Handle(val));
        }
    }
    
    sys_cap_port_grant(0xcf8, 4).expect("sys_cap_port_grant failed");
    sys_cap_port_grant(0xcfc, 4).expect("sys_cap_port_grant failed");
    println!("Hello world!");
    pci_scan();

    let init_system_handle = init_system_handle.expect("missing AUXV_SERENE_INIT_HANDLE");

    let mut discover_builder = IpcPayloadBuilder::with_fixed_size(core::mem::size_of::<IPC_Init_Discover>());
    let discover_name = discover_builder
        .push_bytes(b"vfs_server")
        .expect("failed to append discover server name");
    let packet = IPC_Init_Discover {
        _type: 0,
        _pad: [0; 7],
        name: discover_name,
    };

    let endpoint = sys_endpoint_create().expect("sys_endpoint_create failed");

    discover_builder
        .write_struct(0, &packet)
        .expect("failed to write discover request");
    let discover_payload = discover_builder.finish();
    sys_endpoint_send(init_system_handle, discover_payload.as_slice(), endpoint).expect("sys_endpoint_send failed");

    sys_wait_for(endpoint).expect("sys_wait_for failed");
    let (message_ptr, _total_size) = sys_endpoint_receive(endpoint).expect("sys_endpoint_receive failed");

    unsafe {
        let message= &*message_ptr;
        println!("test_app: received message {:?}", message.payload());
        let payload = message.payload();
        if (message.length as usize) < core::mem::size_of::<IPC_Init_DiscoverResponse>() {
            println!("test_app: invalid discover response size");   
        }
        let reader = IpcPayloadReader::new(payload);
        let response: &IPC_Init_DiscoverResponse = reader.read_struct(0).expect("invalid discover response header");
        println!("test_app: received handle {:?}", response.handle);    
        
        let mut list_req_builder = IpcPayloadBuilder::with_fixed_size(core::mem::size_of::<IPC_VFS_List_Dir_Request>());
        let path = list_req_builder
            .push_bytes(b"/")
            .expect("failed to append list dir path");
        let list_dir_request = IPC_VFS_List_Dir_Request {
            _type: 1,
            _pad: [0; 7],
            path,
        };

        list_req_builder
            .write_struct(0, &list_dir_request)
            .expect("failed to write list dir request");
        let list_req_payload = list_req_builder.finish();
        sys_endpoint_send(response.handle, list_req_payload.as_slice(), endpoint).expect("sys_endpoint_send failed");
        sys_wait_for(endpoint).expect("sys_wait_for failed");
        let (message_ptr, _total_size) = sys_endpoint_receive(endpoint).expect("sys_endpoint_receive failed");
        let message= &*message_ptr;
        let payload = message.payload();
        println!("test_app: received message with payload {:?}", payload);

        if payload[0] == 1 {
            let reader = IpcPayloadReader::new(payload);
            let response: &IPC_VFS_List_Dir_Response = reader.read_struct(0).expect("invalid list dir response header");
            let entries: &[IPC_VFS_List_Dir_Response_Entry] = reader
                .read_array(response.entries)
                .expect("invalid list dir response entries");
            println!("test_app: received list dir response with {} entries", entries.len());
            for (i, entry) in entries.iter().enumerate() {
                let name = reader.read_bytes(entry.name).expect("invalid list dir response entry name");
                let name_str = core::str::from_utf8(name).unwrap_or("");
                println!("test_app: entry {}: name='{}', flags={}", i, name_str, entry.flags);
            }
        } else {
            println!("test_app: received unknown response type {}", payload[0]);
        }

        sys_endpoint_free_message(message_ptr).expect("sys_endpoint_free_message failed");
    }


    sys_exit(0);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("panic: {}", info.message());
    println!("at {:?}", info.location());
    
    sys_exit(1);
}
