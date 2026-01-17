#![no_std]
#![no_main]
use core::panic::PanicInfo;
use serenelib::debug_writer::{_print, DebugWriter};
use serenelib::syscalls::{sys_cap_port_grant, sys_exit, sys_cap_ipc_discovery, sys_endpoint_send};
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

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    sys_cap_port_grant(0xe9, 1).expect("sys_cap_port_grant failed");
    sys_cap_port_grant(0xcf8, 4).expect("sys_cap_port_grant failed");
    sys_cap_port_grant(0xcfc, 4).expect("sys_cap_port_grant failed");
    println!("Hello world!");
    pci_scan();
    let handle = sys_cap_ipc_discovery().expect("sys_cap_ipc_discovery failed");
    sys_endpoint_send(handle, "Hello, world!".as_bytes()).expect("sys_endpoint_send failed");
    sys_exit(0);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("panic: {}", info.message());
    println!("at {:?}", info.location());
    
    sys_exit(1);
}
