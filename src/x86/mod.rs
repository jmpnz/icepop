//! x86-64 thread safe Jit cache for RISCV emulation.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::mmu::VirtAddr;

/// Cache responsible for storing cached jitted blocks and their corresponding
/// translation tables.
///
/// Suppose we compile a block at address 0x4000 then the translation table is
/// the entry at 0x1000 (all instructions are 4 bytes in RISC-V).
///
/// Once a block has been jitted, assembled and cached it can be executed on
/// the fly. The calling convention for jitted code is :
///
/// - Scratch registers (can be clobbered) : (rax, rcx, rdx).
/// - Saved : (rbx since it will store the PC for the guest).
/// - Parameter passing registers : (r8, r9, r10, r11, r12, r13, r14).
/// - Return value (in rax and will return an equivalent exit code).
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub struct JitCache {
    /// Translation tables for the jitted blocks available for execution.
    tlb: Box<[AtomicUsize]>,
    /// Memory that contains the jitted blocks.
    jit: Mutex<(&'static mut [u8], usize)>,
    /// Used memory in the `blocks` space.
    in_use: usize,
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
impl JitCache {
    /// Create a new `JitCache` and allocate backing executable memory for it.
    pub fn new(max_guest_addr: VirtAddr) -> Self {
        Self {
            tlb: (0..(max_guest_addr.0 + 3) / 4)
                .map(|_| AtomicUsize::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            jit: Mutex::new((alloc_rwx(16 * 1024 * 1024), 0)),
            in_use: 0,
        }
    }

    /// Returns a raw address to the translation table.
    pub fn translation_table(&self) -> usize {
        self.tlb.as_ptr() as usize
    }

    /// Returns the number of available jitted blocks.
    pub fn num_blocks(&self) -> usize {
        self.tlb.len()
    }

    /// Lookup the corresponding JIT address for a given guest address using
    /// the translation table.
    /// If the address is not aligned or there is no corresponding JIT address
    /// in case the block at guest address wasn't compiled yet, returns `None`.
    pub fn lookup(&self, addr: VirtAddr) -> Option<usize> {
        // Ensure address is aligned.
        if addr.0 & 3 != 0 {
            return None;
        }

        // We need to ensure that any other stores are visible here, else we
        // will miss this until the next call for the same `addr`.
        let addr = self.tlb.get(addr.0 / 4)?.load(Ordering::SeqCst);
        if addr == 0 {
            None
        } else {
            Some(addr)
        }
    }

    /// Update the jitted block at given `addr` and return its new offset.
    pub fn update(&self, addr: VirtAddr, code: &[u8]) -> usize {
        // Ensure address is aligned.
        assert!(addr.0 & 3 == 0, "Unaligned code address");

        // Get exclusive lock on the JIT cache.
        let mut jit = self.jit.lock().unwrap();

        // Check if there's an existing mapping at this address.
        if let Some(existing) = self.lookup(addr) {
            return existing;
        }

        // Check if we have enough space to store the new block.
        let jit_inuse = jit.1;
        let jit_remain = jit.0.len() - jit_inuse;
        assert!(jit_remain > code.len(), "Out of space, buy more RAM");

        // Store the new code.
        jit.0[jit_inuse..jit_inuse + code.len()].copy_from_slice(code);

        // Compute the new address.
        let new_addr = jit.0[jit_inuse..].as_ptr() as usize;

        // Update the lookup address.
        self.tlb[addr.0 / 4].store(new_addr, Ordering::SeqCst);

        // Update the in use for the JIT.
        jit.1 += code.len();

        println!("Added jit for {:#x} -> {:#x}\n", addr.0, new_addr);
        // Return the new address
        new_addr
    }
}
