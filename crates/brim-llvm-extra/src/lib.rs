//! LLVM calls that inkwell does not wrap safely.
//!
//! Since LLVM 19, debug information about variables is kept in debug
//! records instead of `llvm.dbg.declare` calls. inkwell's
//! `insert_declare_at_end` still returns the record as an instruction,
//! which it is not. [`declare_at_end`] calls LLVM directly and returns
//! nothing. This is the only `unsafe` code in the Brim compiler.
//!
//! The crate is empty without its `llvm` feature.

#![cfg(feature = "llvm")]

use inkwell::basic_block::BasicBlock;
use inkwell::debug_info::{DIExpression, DILocalVariable, DILocation, DebugInfoBuilder};
use inkwell::llvm_sys::debuginfo::LLVMDIBuilderInsertDeclareRecordAtEnd;
use inkwell::values::{AsValueRef, PointerValue};

/// Records that the variable `var` is stored at `storage`, at the end of
/// `block`.
pub fn declare_at_end<'ctx>(
    builder: &DebugInfoBuilder<'ctx>,
    storage: PointerValue<'ctx>,
    var: DILocalVariable<'ctx>,
    expr: DIExpression<'ctx>,
    location: DILocation<'ctx>,
    block: BasicBlock<'ctx>,
) {
    // SAFETY: every argument is a live LLVM object of the kind the function
    // expects, owned by the same context, and the returned record is not
    // used.
    unsafe {
        LLVMDIBuilderInsertDeclareRecordAtEnd(
            builder.as_mut_ptr(),
            storage.as_value_ref(),
            var.as_mut_ptr(),
            expr.as_mut_ptr(),
            location.as_mut_ptr(),
            block.as_mut_ptr(),
        );
    }
}
