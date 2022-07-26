/// PCI VirtIO Device Register Interface.
///
/// Ref: VirtIO spec v1.1 section 4.1.4
/// Ref: VirtQueue Legacy Interface section 2.6.2

use bitflags::*;
use volatile::{ReadOnly, Volatile};
use crate::header::DeviceType;
use pci::BAR;
use log::*;

/// Common Configuration, cfg_type=0x1(VIRTIO_PCI_CAP_COMMON_CFG).
/// See VirtIO spec section 4.1.4.3
#[repr(C)]
pub struct VirtIOPCICommonCfgRaw {
    /* About the whole device */
    device_features_sel: Volatile<u32>,
    device_features: ReadOnly<u32>,
    driver_features_sel: Volatile<u32>,
    driver_features: Volatile<u32>,
    msix_config: Volatile<u16>,
    num_queues: ReadOnly<u16>,
    device_status: Volatile<DeviceStatusU8>,
    config_generation: ReadOnly<u8>,

    /* About a specific virtqueue */
    queue_sel: Volatile<u16>,
    queue_size: Volatile<u16>,
    queue_msix_vector: Volatile<u16>,
    queue_enable: Volatile<u16>,
    queue_notify_off: ReadOnly<u16>,
    queue_desc: Volatile<u64>,
    queue_driver: Volatile<u64>,
    queue_device: Volatile<u64>,
}

/// See VirtIO spec 4.1.4.
#[repr(C)]
pub struct VirtIOPCICapRaw {
    cap_vndr: Volatile<u8>,
    cap_next: Volatile<u8>,
    cap_len: Volatile<u8>,
    cfg_type: Volatile<u8>,
    bar: Volatile<u8>,
    padding: [u8; 3],
    offset: Volatile<u32>,
    length: Volatile<u32>,
}

/// See VirtIO spec 4.1.4.4.
#[repr(C)]
pub struct VirtIOPCINotifyCapRaw {
    cap: VirtIOPCICapRaw,
    nofity_off_multiplier: Volatile<u32>,
}

/// All information required by a virtio pci device.
pub struct VirtIOPCIHeader {
    device_id: u16,
    bars: [Option<BAR>; 6],
    common_cfg: &'static mut VirtIOPCICommonCfgRaw,
    notify_cap: &'static mut VirtIOPCINotifyCapRaw,
    device_cfg_addr: usize,
}

impl VirtIOPCIHeader {
    /// Create a VirtIOPCIHeader.
    /// Safety: Caller must guarantee the correctness of `common_cfg_base_addr` and 
    /// `notify_cap_base_addr`.
    pub unsafe fn new(
        device_id: u16,
        bars: [Option<BAR>; 6],
        common_cfg_base_addr: u64,
        notify_cap_base_addr: u64,
        device_cfg_base_addr: u64,
    ) -> Self {
        Self {
            device_id,
            bars,
            common_cfg: &mut *(common_cfg_base_addr as *mut VirtIOPCICommonCfgRaw),
            notify_cap: &mut *(notify_cap_base_addr as *mut VirtIOPCINotifyCapRaw),
            device_cfg_addr: device_cfg_base_addr as usize,
        }
    }

    /// Device type of this virtio-pci device.
    pub fn device_type(&self) -> DeviceType {
        match self.device_id {
            0x1000 => DeviceType::Network,
            0x1001 => DeviceType::Block,
            0x1002 => DeviceType::MemoryBallooning,
            0x1003 => DeviceType::Console,
            0x1004 => DeviceType::ScsiHost,
            0x1005 => DeviceType::EntropySource,
            0x1009 => DeviceType::_9P,
            _ => {
                panic!("Unknown virtio device type, pci device_id = {}", self.device_id);
            }
        }
    }

    /// Begin initializing the device.
    ///
    /// Ref: virtio 3.1.1 Device Initialization
    pub fn begin_init(&mut self, negotiate_features: impl FnOnce(u64) -> u64) {
        self.common_cfg.device_status.write(DeviceStatusU8::ACKNOWLEDGE);
        self.common_cfg.device_status.write(DeviceStatusU8::DRIVER);

        let features = self.read_device_features();
        self.write_driver_features(negotiate_features(features));
        self.common_cfg.device_status.write(DeviceStatusU8::FEATURES_OK);
        let status = self.common_cfg.device_status.read();
        if !status.contains(DeviceStatusU8::FEATURES_OK) {
            panic!("virtio pci device initialization failed");
        }
    }

    /// Finish initializing the device.
    pub fn finish_init(&mut self) {
        self.common_cfg.device_status.write(DeviceStatusU8::DRIVER_OK);
    }

    /// Read device features.
    fn read_device_features(&mut self) -> u64 {
        self.common_cfg.device_features_sel.write(0); // device features [0, 32)
        let mut device_features_bits = self.common_cfg.device_features.read().into();
        self.common_cfg.device_features_sel.write(1); // device features [32, 64)
        device_features_bits += (self.common_cfg.device_features.read() as u64) << 32;
        device_features_bits
    }

    /// Write device features.
    fn write_driver_features(&mut self, driver_features: u64) {
        self.common_cfg.driver_features_sel.write(0); // driver features [0, 32)
        self.common_cfg.driver_features.write(driver_features as u32);
        self.common_cfg.driver_features_sel.write(1); // driver features [32, 64)
        self.common_cfg.driver_features.write((driver_features >> 32) as u32);
    }

    /// Whether the queue is in used.
    pub fn queue_used(&mut self, queue: u32) -> bool {
        self.common_cfg.queue_sel.write(queue as u16);
        self.common_cfg.queue_desc.read() != 0
            || self.common_cfg.queue_driver.read() != 0
            || self.common_cfg.queue_device.read() != 0
    }

    /// Get the max size of queue.
    pub fn max_queue_size(&self) -> u32 {
        self.common_cfg.queue_size.read() as u32
    }

    /// Set queue.
    pub fn queue_set(&mut self, queue: u32, size: u32, desc_table_paddr: u64, avail_paddr: u64, used_paddr: u64) {
        self.common_cfg.queue_sel.write(queue as u16);
        // Do not use legacy interface, thus we can negotiate the queue_size(equal to or lower than)
        self.common_cfg.queue_size.write(size as u16);
        self.common_cfg.queue_desc.write(desc_table_paddr as u64);
        self.common_cfg.queue_driver.write(avail_paddr as u64);
        self.common_cfg.queue_device.write(used_paddr as u64);
    }

    /// Enable the current VirtQueue.
    /// According the VirtIO spec 4.1.4.3.2, all other VirtQueue fields should be set up
    /// before enabling the VirtQueue.
    pub fn queue_enable(&mut self) {
        info!("queue_enable={}", self.common_cfg.queue_enable.read());
        self.common_cfg.queue_enable.write(0x1);
        info!("queue_enable={}", self.common_cfg.queue_enable.read());
    }

    /// Return the notify address of the current VirtQueue.
    /// It can be used by the driver to notify the device.
    /// Ref: VirtIO spec v1.1 section 4.1.4.4
    fn queue_notify_address(&self) -> usize {
        let bar_idx = self.notify_cap.cap.bar.read() as usize;
        if let Some(bar) = self.bars[bar_idx] {
            let bar_base_addr = match bar {
                BAR::Memory(addr, _, _, _) => addr as usize,
                BAR::IO(addr, _) => addr as usize,
            };
            
            let cap_offset = self.notify_cap.cap.offset.read() as usize;
            let queue_notify_off = self.common_cfg.queue_notify_off.read() as usize;
            let notify_off_mul = self.notify_cap.nofity_off_multiplier.read() as usize;
            info!("cap_off={:#x},notify_off={:#x},mul={:#x}", cap_offset, queue_notify_off, notify_off_mul);
            bar_base_addr + cap_offset + queue_notify_off * notify_off_mul
        } else {
            panic!("BAR {} does not exist!", bar_idx);
        }
    }

    /// Notify the device that a new request has been submitted.
    /// Assuming that VIRTIO_F_NOTIFICATION_DATA has not been negotiated.
    /// Ref: VirtIO spec v1.1 section 4.1.5.2
    pub fn notify(&mut self, queue_idx: u16) {
        // Safety: The implementation of `queue_notify_address` needs to be correct.
        unsafe {
            (self.queue_notify_address() as *mut u16).write_volatile(queue_idx);
        }
    }

    /// Returns the address fo the device-specific configuration.
    pub fn config_space(&self) -> usize {
        self.device_cfg_addr
    }

}

bitflags! {
    /// The device status field.
    pub struct DeviceStatusU8: u8 {
        /// Indicates that the guest OS has found the device and recognized it
        /// as a valid virtio device.
        const ACKNOWLEDGE = 1;

        /// Indicates that the guest OS knows how to drive the device.
        const DRIVER = 2;

        /// Indicates that something went wrong in the guest, and it has given
        /// up on the device. This could be an internal error, or the driver
        /// didn’t like the device for some reason, or even a fatal error
        /// during device operation.
        const FAILED = 128;

        /// Indicates that the driver has acknowledged all the features it
        /// understands, and feature negotiation is complete.
        const FEATURES_OK = 8;

        /// Indicates that the driver is set up and ready to drive the device.
        const DRIVER_OK = 4;

        /// Indicates that the device has experienced an error from which it
        /// can’t recover.
        const DEVICE_NEEDS_RESET = 64;
    }
}