// Copyright (c) 2013-2015 Sandstorm Development Group, Inc. and contributors
// Licensed under the MIT License:
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

use crate::private::layout::CapTable;
use crate::private::layout::{
    ListReader, PointerBuilder, PointerReader, StructBuilder, StructReader, StructSize,
};
use crate::Result;

use core::marker::PhantomData;

pub trait HasStructSize {
    const STRUCT_SIZE: StructSize;
}

/// Trait for all types that can be converted to a low-level `StructReader`.
pub trait IntoInternalStructReader<'a> {
    fn into_internal_struct_reader(self) -> StructReader<'a>;
}

/// Trait for all types that can be converted to a low-level `StructBuilder`.
pub trait IntoInternalStructBuilder<'a> {
    fn into_internal_struct_builder(self) -> StructBuilder<'a>;
}

/// The offset a [`UseSiteTable`] gives for a member that is not mapped at
/// that use site (an incomplete `@[...]`, or a member the table's schema
/// version doesn't have).
pub const UNMAPPED: u32 = 0xffff_ffff;

/// One entry of a [`UseSiteTable`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseSiteEntry<'t> {
    /// A slot member: its offset in the parent's data or pointer section,
    /// in units of the member's type, or [`UNMAPPED`].
    Offset(u32),
    /// A member that is itself a group or union newtype: its own table.
    Nested(&'t UseSiteTable<'t>),
}

/// Where one use site of a group or union `type` newtype places the
/// newtype's members within the parent struct.
///
/// There is one table per template level. `entries` is indexed by a
/// member's position in the newtype template's `fields` list, which is
/// stable under schema evolution (new fields are appended). A table built
/// from an older or newer version of the newtype than the carrier reading
/// it may therefore be shorter or longer: an index past the end is
/// unmapped, and extra entries are ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UseSiteTable<'t> {
    /// Indexed by template field position.
    pub entries: &'t [UseSiteEntry<'t>],
    /// For a union newtype: the offset of its discriminant, in 16-bit
    /// units. [`UNMAPPED`] if the union itself is not mapped. Unused by
    /// non-union newtypes.
    pub discriminant_offset: u32,
}

/// The table of a use site where nothing is mapped.
pub static EMPTY_USE_SITE_TABLE: UseSiteTable<'static> = UseSiteTable {
    entries: &[],
    discriminant_offset: UNMAPPED,
};

impl<'t> UseSiteTable<'t> {
    /// The offset of the slot member at template field position `index`, or
    /// [`UNMAPPED`] if it isn't mapped at this use site.
    #[inline]
    pub fn offset(&self, index: usize) -> u32 {
        match self.entries.get(index) {
            Some(UseSiteEntry::Offset(offset)) => *offset,
            _ => UNMAPPED,
        }
    }

    /// The table of the nested-newtype member at template field position
    /// `index`, or `None` if it isn't mapped at this use site.
    #[inline]
    pub fn get_nested(&self, index: usize) -> Option<&'t UseSiteTable<'t>> {
        match self.entries.get(index) {
            Some(UseSiteEntry::Nested(table)) => Some(table),
            _ => None,
        }
    }

    /// Like [`get_nested`](Self::get_nested), but an unmapped member gets
    /// [`EMPTY_USE_SITE_TABLE`], over which every read gives the default.
    #[inline]
    pub fn nested(&self, index: usize) -> &'t UseSiteTable<'t> {
        self.get_nested(index).unwrap_or(&EMPTY_USE_SITE_TABLE)
    }
}

/// The reader every group or union `type` newtype generates: one type that
/// reads the newtype at any of its use sites.
///
/// Inlining lets each struct place a newtype's members wherever they fit,
/// so a use site is described by the parent's raw reader plus a
/// [`UseSiteTable`] saying where each member lives. Typed code gets one
/// from a use site's `as_any()`. Dynamic code gets one with
/// `dynamic_value::Reader::downcast::<vec3::AnyReader>()` on the group,
/// which reads the same table from the schema.
pub trait AnyReader<'a>: Sized {
    /// The id of the `type` declaration this reads (not of an alias of it).
    const TYPE_ID: u64;

    /// Builds the reader over one use site. Generated code and `downcast`
    /// call this; other code normally doesn't.
    fn from_use_site(reader: StructReader<'a>, table: &'a UseSiteTable<'a>) -> Self;
}

/// The builder counterpart of [`AnyReader`].
pub trait AnyBuilder<'a>: Sized {
    /// The id of the `type` declaration this writes (not of an alias of it).
    const TYPE_ID: u64;

    /// Builds the builder over one use site; see
    /// [`AnyReader::from_use_site`].
    fn from_use_site(builder: StructBuilder<'a>, table: &'a UseSiteTable<'a>) -> Self;
}

/// Trait for all types that can be converted to a low-level `ListReader`.
pub trait IntoInternalListReader<'a> {
    fn into_internal_list_reader(self) -> ListReader<'a>;
}

pub trait FromPointerReader<'a>: Sized {
    fn get_from_pointer(
        reader: &PointerReader<'a>,
        default: Option<&'a [crate::Word]>,
    ) -> Result<Self>;
}

/// A trait to encode relationships between readers and builders.
///
/// If `Foo` is a Cap'n Proto struct and `Bar` is a Rust-native struct, then
/// `foo::Reader<'a>` is to `foo::Owned` as `&'a Bar` is to `Bar`, and
/// `foo::Builder<'a>` is to `foo::Owned` as `&'a mut Bar` is to `Bar`.
/// The relationship is formalized by an `impl capnp::traits::Owned for foo::Owned`.
/// Because Cap'n Proto struct layout differs from Rust struct layout, a `foo::Owned` value
/// cannot be used for anything interesting on its own; the `foo::Owned` type is useful
/// nonetheless as a type parameter, e.g. for a generic container that owns a Cap'n Proto
/// message of type `T: capnp::traits::Owned`.
pub trait Owned: crate::introspect::Introspect {
    type Reader<'a>: FromPointerReader<'a> + SetterInput<Self>;
    type Builder<'a>: FromPointerBuilder<'a>;
}

pub trait OwnedStruct: crate::introspect::Introspect {
    type Reader<'a>: From<StructReader<'a>> + SetterInput<Self> + IntoInternalStructReader<'a>;
    type Builder<'a>: From<StructBuilder<'a>> + HasStructSize;
}

pub trait Pipelined {
    type Pipeline;
}

pub trait FromPointerBuilder<'a>: Sized {
    fn init_pointer(builder: PointerBuilder<'a>, length: u32) -> Self;
    fn get_from_pointer(
        builder: PointerBuilder<'a>,
        default: Option<&'a [crate::Word]>,
    ) -> Result<Self>;
}

/// A trait marking types that can be passed as inputs to setter methods.
/// `Receiver` is intended to be an `Owned`, representing the destination type.
///
/// This trait allows setters to support multiple types of input. For example,
/// a text field setter accepts values of type `&str` and of type `text::Reader`.
pub trait SetterInput<Receiver: ?Sized> {
    /// Copies the values from `input` into `builder`, where `builder`
    /// represents the backing memory of a `<Receiver as Owned>::Builder`.
    ///
    /// End user code should never need to call this method directly.
    fn set_pointer_builder(
        builder: PointerBuilder<'_>,
        input: Self,
        canonicalize: bool,
    ) -> Result<()>;
}

/// A trait for types that can be "imbued" with capabilities.
///
/// A newly-read message from the network might contain capability pointers
/// but until the message has been imbued with the actual capabilities,
/// those pointers will not be usable.
pub trait Imbue<'a> {
    fn imbue(&mut self, caps: &'a CapTable);
}

/// Like `Imbue`, but the capability table is mutable.
pub trait ImbueMut<'a> {
    fn imbue_mut(&mut self, caps: &'a mut CapTable);
}

/// User-defined Cap'n Proto structs and interfaces are statically assigned a
/// 64-bit type ID. This trait allows the ID to be retrieved.
pub trait HasTypeId {
    const TYPE_ID: u64;
}

pub trait IndexMove<I, T> {
    fn index_move(&self, index: I) -> T;
}

pub struct ListIter<T, U> {
    marker: PhantomData<U>,
    list: T,
    index: u32,
    size: u32,
}

impl<T, U> ListIter<T, U> {
    pub fn new(list: T, size: u32) -> Self {
        Self {
            list,
            index: 0,
            size,
            marker: PhantomData,
        }
    }
}

impl<U, T: IndexMove<u32, U>> ::core::iter::Iterator for ListIter<T, U> {
    type Item = U;
    fn next(&mut self) -> ::core::option::Option<U> {
        if self.index < self.size {
            let result = self.list.index_move(self.index);
            self.index += 1;
            Some(result)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.size as usize - self.index as usize;
        (remaining, Some(remaining))
    }

    fn nth(&mut self, p: usize) -> Option<U> {
        let Some(p) = p.try_into().ok() else {
            self.index = self.size;
            return None;
        };
        let Some(nth_index) = self.index.checked_add(p) else {
            self.index = self.size;
            return None;
        };
        if nth_index < self.size {
            self.index = nth_index;
            let result = self.list.index_move(self.index);
            self.index += 1;
            Some(result)
        } else {
            self.index = self.size;
            None
        }
    }
}

impl<U, T: IndexMove<u32, U>> ::core::iter::ExactSizeIterator for ListIter<T, U> {
    fn len(&self) -> usize {
        self.size as usize - self.index as usize
    }
}

impl<U, T: IndexMove<u32, U>> ::core::iter::DoubleEndedIterator for ListIter<T, U> {
    fn next_back(&mut self) -> ::core::option::Option<U> {
        if self.size > self.index {
            self.size -= 1;
            Some(self.list.index_move(self.size))
        } else {
            None
        }
    }
}

/// Iterator for a list whose indices are of type `u16`.
pub struct ShortListIter<T, U> {
    marker: PhantomData<U>,
    list: T,
    index: u16,
    size: u16,
}

impl<T, U> ShortListIter<T, U> {
    pub fn new(list: T, size: u16) -> Self {
        Self {
            list,
            index: 0,
            size,
            marker: PhantomData,
        }
    }
}

impl<U, T: IndexMove<u16, U>> ::core::iter::Iterator for ShortListIter<T, U> {
    type Item = U;
    fn next(&mut self) -> ::core::option::Option<U> {
        if self.index < self.size {
            let result = self.list.index_move(self.index);
            self.index += 1;
            Some(result)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.size as usize - self.index as usize;
        (remaining, Some(remaining))
    }

    fn nth(&mut self, p: usize) -> Option<U> {
        let Some(p) = p.try_into().ok() else {
            self.index = self.size;
            return None;
        };
        let Some(nth_index) = self.index.checked_add(p) else {
            self.index = self.size;
            return None;
        };
        if nth_index < self.size {
            self.index = nth_index;
            let result = self.list.index_move(self.index);
            self.index += 1;
            Some(result)
        } else {
            self.index = self.size;
            None
        }
    }
}

impl<U, T: IndexMove<u16, U>> ::core::iter::ExactSizeIterator for ShortListIter<T, U> {
    fn len(&self) -> usize {
        self.size as usize - self.index as usize
    }
}

impl<U, T: IndexMove<u16, U>> ::core::iter::DoubleEndedIterator for ShortListIter<T, U> {
    fn next_back(&mut self) -> ::core::option::Option<U> {
        if self.size > self.index {
            self.size -= 1;
            Some(self.list.index_move(self.size))
        } else {
            None
        }
    }
}
