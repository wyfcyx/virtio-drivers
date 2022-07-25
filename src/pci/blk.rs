use super::VirtIOPCIHeader;
use crate::queue::VirtQueue;
use crate::blk::*;
use crate::{Result, AsBuf, Error};
use log::*;
use core::hint::spin_loop;

/// The virtio block device is a simple virtual block device (ie. disk) which is
/// connected to a PCI bus.
///
/// Read and write requests (and other exotic requests) are placed in the queue,
/// and serviced (probably out of order) by the device except where noted.
pub struct VirtIOBlkPCI<'a> {
    header: VirtIOPCIHeader,
    queue: VirtQueue<'a>,
    capacity: usize,
}

impl<'a> VirtIOBlkPCI<'a> {
    /// Create a new VirtIO-Blk PCI driver.
    pub fn new(mut header: VirtIOPCIHeader) -> Result<Self> {
        header.begin_init(|features| {
            let features = BlkFeature::from_bits_truncate(features);
            info!("device features: {:?}", features);
            // negotiate these flags only
            let supported_features = BlkFeature::empty();
            (features & supported_features).bits()
        });

        // read configuration space
        let config = unsafe { &mut *(header.config_space() as *mut BlkConfig) };
        info!("config: {:?}", config);
        info!(
            "found a block device of size {}KB",
            config.capacity.read() / 2
        );

        let queue = VirtQueue::new_pci(&mut header, 0)?;
        header.finish_init();

        Ok(Self {
            header,
            queue,
            capacity: config.capacity.read() as usize,
        })
    }

    /// Acknowledge interrupt.
    pub fn ack_interrupt(&mut self) -> bool {
        unimplemented!()
    }

    /// Read a block.
    pub fn read_block(&mut self, block_id: usize, buf: &mut [u8]) -> Result {
        info!("reading block {:#x}", block_id);
        assert_eq!(buf.len(), BLK_SIZE);
        let req = BlkReq::new(ReqType::In, 0, block_id as u64);
        let mut resp = BlkResp::default();
        info!("before adding");
        self.queue.add(&[req.as_buf()], &[buf, resp.as_buf_mut()])?;
        info!("before notifying");
        self.header.notify(0);
        info!("after notifying");
        while !self.queue.can_pop() {
            spin_loop();
        }
        self.queue.pop_used()?;
        info!("poped!");
        match resp.status() {
            RespStatus::Ok => Ok(()),
            _ => Err(Error::IoError),
        }
    }

    /// Write a block.
    pub fn write_block(&mut self, block_id: usize, buf: &[u8]) -> Result {
        assert_eq!(buf.len(), BLK_SIZE);
        let req = BlkReq::new(ReqType::Out, 0, block_id as u64);
        let mut resp = BlkResp::default();
        self.queue.add(&[req.as_buf(), buf], &[resp.as_buf_mut()])?;
        self.header.notify(0);
        while !self.queue.can_pop() {
            spin_loop();
        }
        self.queue.pop_used()?;
        match resp.status() {
            RespStatus::Ok => Ok(()),
            _ => Err(Error::IoError),
        }
    }

}