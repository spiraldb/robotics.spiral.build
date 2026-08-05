//! A Rust interface for Objective-C blocks.
//!
//! This is `block` 0.1.6 with its opaque class representation corrected for
//! Rust's `uninhabited_static` future-compatibility lint. The Blocks runtime
//! exports each concrete class as storage for 32 pointers.

use std::marker::PhantomData;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::os::raw::{c_int, c_ulong, c_void};
use std::ptr;

#[repr(C)]
struct Class {
    _private: [*mut c_void; 32],
}

#[cfg_attr(
    any(target_os = "macos", target_os = "ios"),
    link(name = "System", kind = "dylib")
)]
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios")),
    link(name = "BlocksRuntime", kind = "dylib")
)]
extern "C" {
    static _NSConcreteStackBlock: Class;

    fn _Block_copy(block: *const c_void) -> *mut c_void;
    fn _Block_release(block: *const c_void);
}

/// Types that may be used as the arguments to an Objective-C block.
pub trait BlockArguments: Sized {
    /// Calls the given block with `self` as its arguments.
    ///
    /// The pointer must refer to a valid block with the matching signature.
    unsafe fn call_block<R>(self, block: *mut Block<Self, R>) -> R;
}

macro_rules! block_args_impl {
    ($($argument:ident : $argument_type:ident),*) => {
        impl<$($argument_type),*> BlockArguments for ($($argument_type,)*) {
            unsafe fn call_block<R>(self, block: *mut Block<Self, R>) -> R {
                let invoke: unsafe extern "C" fn(
                    *mut Block<Self, R>
                    $(, $argument_type)*
                ) -> R = {
                    let base = block as *mut BlockBase<Self, R>;
                    mem::transmute((*base).invoke)
                };
                let ($($argument,)*) = self;
                invoke(block $(, $argument)*)
            }
        }
    };
}

block_args_impl!();
block_args_impl!(a: A);
block_args_impl!(a: A, b: B);
block_args_impl!(a: A, b: B, c: C);
block_args_impl!(a: A, b: B, c: C, d: D);
block_args_impl!(a: A, b: B, c: C, d: D, e: E);
block_args_impl!(a: A, b: B, c: C, d: D, e: E, f: F);
block_args_impl!(a: A, b: B, c: C, d: D, e: E, f: F, g: G);
block_args_impl!(a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H);
block_args_impl!(a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I);
block_args_impl!(a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J);
block_args_impl!(a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J, k: K);
block_args_impl!(a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J, k: K, l: L);

#[repr(C)]
struct BlockBase<A, R> {
    isa: *const Class,
    flags: c_int,
    reserved: c_int,
    invoke: unsafe extern "C" fn(*mut Block<A, R>, ...) -> R,
}

/// An Objective-C block that takes arguments of `A` and returns `R`.
#[repr(C)]
pub struct Block<A, R> {
    base: PhantomData<BlockBase<A, R>>,
}

impl<A: BlockArguments, R> Block<A, R> {
    /// Calls this block with the supplied arguments.
    ///
    /// The block must be valid and its foreign implementation must uphold
    /// Rust's safety requirements.
    pub unsafe fn call(&self, arguments: A) -> R {
        arguments.call_block(self as *const _ as *mut _)
    }
}

/// A reference-counted Objective-C block.
pub struct RcBlock<A, R> {
    pointer: *mut Block<A, R>,
}

impl<A, R> RcBlock<A, R> {
    /// Constructs an `RcBlock` without copying it.
    ///
    /// `pointer` must refer to a valid block with a +1 reference count.
    pub unsafe fn new(pointer: *mut Block<A, R>) -> Self {
        Self { pointer }
    }

    /// Copies a valid Objective-C block into reference-counted storage.
    pub unsafe fn copy(pointer: *mut Block<A, R>) -> Self {
        let pointer = _Block_copy(pointer.cast()).cast();
        Self { pointer }
    }
}

impl<A, R> Clone for RcBlock<A, R> {
    fn clone(&self) -> Self {
        unsafe { Self::copy(self.pointer) }
    }
}

impl<A, R> Deref for RcBlock<A, R> {
    type Target = Block<A, R>;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.pointer }
    }
}

impl<A, R> Drop for RcBlock<A, R> {
    fn drop(&mut self) {
        unsafe { _Block_release(self.pointer.cast()) }
    }
}

/// Types that may be converted into a `ConcreteBlock`.
pub trait IntoConcreteBlock<A>: Sized
where
    A: BlockArguments,
{
    /// The resulting block's return type.
    type Ret;

    /// Consumes this closure and creates a concrete block.
    fn into_concrete_block(self) -> ConcreteBlock<A, Self::Ret, Self>;
}

macro_rules! concrete_block_impl {
    ($function:ident) => {
        concrete_block_impl!($function,);
    };
    ($function:ident, $($argument:ident : $argument_type:ident),*) => {
        impl<$($argument_type,)* R, Closure> IntoConcreteBlock<($($argument_type,)*)> for Closure
        where
            Closure: Fn($($argument_type,)*) -> R,
        {
            type Ret = R;

            fn into_concrete_block(
                self,
            ) -> ConcreteBlock<($($argument_type,)*), R, Closure> {
                unsafe extern "C" fn $function<$($argument_type,)* R, Closure>(
                    block_pointer: *mut ConcreteBlock<($($argument_type,)*), R, Closure>
                    $(, $argument: $argument_type)*
                ) -> R
                where
                    Closure: Fn($($argument_type,)*) -> R,
                {
                    let block = &*block_pointer;
                    (block.closure)($($argument),*)
                }

                let function: unsafe extern "C" fn(
                    *mut ConcreteBlock<($($argument_type,)*), R, Closure>
                    $(, $argument_type)*
                ) -> R = $function;
                unsafe { ConcreteBlock::with_invoke(mem::transmute(function), self) }
            }
        }
    };
}

concrete_block_impl!(concrete_block_invoke_args0);
concrete_block_impl!(concrete_block_invoke_args1, a: A);
concrete_block_impl!(concrete_block_invoke_args2, a: A, b: B);
concrete_block_impl!(concrete_block_invoke_args3, a: A, b: B, c: C);
concrete_block_impl!(concrete_block_invoke_args4, a: A, b: B, c: C, d: D);
concrete_block_impl!(concrete_block_invoke_args5, a: A, b: B, c: C, d: D, e: E);
concrete_block_impl!(concrete_block_invoke_args6, a: A, b: B, c: C, d: D, e: E, f: F);
concrete_block_impl!(concrete_block_invoke_args7, a: A, b: B, c: C, d: D, e: E, f: F, g: G);
concrete_block_impl!(concrete_block_invoke_args8, a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H);
concrete_block_impl!(concrete_block_invoke_args9, a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I);
concrete_block_impl!(concrete_block_invoke_args10, a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J);
concrete_block_impl!(concrete_block_invoke_args11, a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J, k: K);
concrete_block_impl!(concrete_block_invoke_args12, a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J, k: K, l: L);

/// An Objective-C block whose size is known at compile time.
#[repr(C)]
pub struct ConcreteBlock<A, R, Closure> {
    base: BlockBase<A, R>,
    descriptor: Box<BlockDescriptor<ConcreteBlock<A, R, Closure>>>,
    closure: Closure,
}

impl<A, R, Closure> ConcreteBlock<A, R, Closure>
where
    A: BlockArguments,
    Closure: IntoConcreteBlock<A, Ret = R>,
{
    /// Creates a concrete block from a closure.
    pub fn new(closure: Closure) -> Self {
        closure.into_concrete_block()
    }
}

impl<A, R, Closure> ConcreteBlock<A, R, Closure> {
    unsafe fn with_invoke(
        invoke: unsafe extern "C" fn(*mut Self, ...) -> R,
        closure: Closure,
    ) -> Self {
        Self {
            base: BlockBase {
                isa: &_NSConcreteStackBlock,
                flags: 1 << 25,
                reserved: 0,
                invoke: mem::transmute(invoke),
            },
            descriptor: Box::new(BlockDescriptor::new()),
            closure,
        }
    }
}

impl<A, R, Closure> ConcreteBlock<A, R, Closure>
where
    Closure: 'static,
{
    /// Copies this stack block into reference-counted heap storage.
    pub fn copy(self) -> RcBlock<A, R> {
        unsafe {
            let mut block = self;
            let copied = RcBlock::copy(&mut *block);
            mem::forget(block);
            copied
        }
    }
}

impl<A, R, Closure> Clone for ConcreteBlock<A, R, Closure>
where
    Closure: Clone,
{
    fn clone(&self) -> Self {
        unsafe { Self::with_invoke(mem::transmute(self.base.invoke), self.closure.clone()) }
    }
}

impl<A, R, Closure> Deref for ConcreteBlock<A, R, Closure> {
    type Target = Block<A, R>;

    fn deref(&self) -> &Self::Target {
        unsafe { &*(&self.base as *const _ as *const Block<A, R>) }
    }
}

impl<A, R, Closure> DerefMut for ConcreteBlock<A, R, Closure> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *(&mut self.base as *mut _ as *mut Block<A, R>) }
    }
}

unsafe extern "C" fn block_context_dispose<BlockType>(block: &mut BlockType) {
    ptr::read(block);
}

unsafe extern "C" fn block_context_copy<BlockType>(
    _destination: &mut BlockType,
    _source: &BlockType,
) {
}

#[repr(C)]
struct BlockDescriptor<BlockType> {
    reserved: c_ulong,
    block_size: c_ulong,
    copy_helper: unsafe extern "C" fn(&mut BlockType, &BlockType),
    dispose_helper: unsafe extern "C" fn(&mut BlockType),
}

impl<BlockType> BlockDescriptor<BlockType> {
    fn new() -> Self {
        Self {
            reserved: 0,
            block_size: mem::size_of::<BlockType>() as c_ulong,
            copy_helper: block_context_copy::<BlockType>,
            dispose_helper: block_context_dispose::<BlockType>,
        }
    }
}
