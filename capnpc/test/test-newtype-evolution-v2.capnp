# Copyright (c) 2024 the Cap'n Proto authors and contributors
# Licensed under the MIT License:
#
# Permission is hereby granted, free of charge, to any person obtaining a copy
# of this software and associated documentation files (the "Software"), to deal
# in the Software without restriction, including without limitation the rights
# to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
# copies of the Software, and to permit persons to whom the Software is
# furnished to do so, subject to the following conditions:
#
# The above copyright notice and this permission notice shall be included in
# all copies or substantial portions of the Software.
#
# THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
# IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
# FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
# AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
# LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
# OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
# THE SOFTWARE.


# Version 2 of test-newtype-evolution-v1.capnp (see there): every newtype gains a field. New
# nested-newtype fields are mapped only where tree order and ordinal order agree, which every
# compiler version stamps the same way.

@0xacb70ed466e3643e;

type Vec @0x8187024fc908caf2 = group {
  x @0 :Float32;
  y @1 :Float32;
  z @2 :Float32;                             # new
}

type Labeled @0x8c5f861a1e8b22a0 = group {
  id @0 :UInt32;
  pos @[1-2, 3] :Vec;                        # maps the new Vec.z
  name @4 :Text;                             # new
}

type Shape @0xc66a271a0a79ca44 = union {
  none @0 :Void;
  circle @1 :Float32;
  point @[2-3, 4] :Vec;                      # maps the new Vec.z
  square @5 :Float32;                        # new arm
}

type Span @0xb5be055982f51547 = group {      # unchanged, so Vec.z is unmapped in both members
  tl @[0-1] :Vec;
  br @[2-3] :Vec;
}

struct Holder {
  a @[0-1, 14] :Vec;
  tag @2 :UInt32;
  l @[3-5, 15, 16] :Labeled;
  s @[6-9, 17, 18] :Shape;
  r @[10-13] :Span;
}

# The repro from plans/newtype-evolution-fixes.md (see test-newtype-evolution-v1.capnp).
type Vec3 @0x85009e75d71f925b = group { x @0 :Float32; y @1 :Float32; z @2 :Float32; w @3 :Float32; }
type Rect @0xbb1e31f2f3748672 = group { tl @[0-2,6] :Vec3; br @[3-5,7] :Vec3; }
struct S { r @[0-5,7,8] :Rect; tag @6 :UInt32; }
