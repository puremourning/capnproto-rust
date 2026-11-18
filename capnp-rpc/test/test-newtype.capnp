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

# Exercises `type` newtypes in interface method parameter/result slots, driven through a real RPC
# call in newtype_test.rs. Requires a newtype-capable `capnp` compiler on PATH.

@0xaa9876414f6b26b1;

type Vec3 = group { x @0 :Float32; y @1 :Float32; z @2 :Float32; }
type Uuid = Data;
type Age = UInt16;
type Price = group { value @0 :Int64; scale @1 :UInt16; }
type OrderType = union {
  limit @[0-1] :Price;
  market @2 :Void;
  cancel @3 :Int32;
}

struct PlaceParams {
  # A method's parameters are an ordinary struct, so group/union newtypes reach a method through a
  # named parameter struct like this one. (The `@[...]` ordinal-mapping syntax is only valid on
  # struct fields, not inside an inline `(...)` parameter list.)
  spot @[0-2] :Vec3;                    # group newtype in a request slot
  tag @3 :Uuid;                         # scalar pointer newtype in a request slot
}

struct PlaceResults {
  echo @[0-2] :Vec3;                    # group newtype in a response slot
  kind @[3, 4, 5, 6] :OrderType;        # union newtype in a response slot
}

type Endpoint = group { name @0 :Text; target @1 :Registry; }
                                        # group newtype carrying a capability

struct ResolveParams {
  # A capability inside a group newtype in a request: the callee reads it back out of the
  # request's cap table through the newtype accessors.
  ep @[0-1] :Endpoint;
}

struct ResolveResults {
  # ... and in a response, where it is written into the results' cap table.
  ep @[0-1] :Endpoint;
}

# Aliases of existing interfaces are transparent (like C++'s `using`): `handle::Client` is
# `registry::Client`. `Handle` is declared before its target on purpose.
type Handle = Registry;
interface Holder(T) { get @0 () -> (value :T); }
type TextHolder = Holder(Text);         # branded interface alias

interface Registry {
  # Newtypes in interface method parameter/result slots. Scalar newtypes may appear directly in an
  # inline parameter list; group/union newtypes come in via the named structs above.
  lookup @0 (id :Uuid, age :Age) -> (foundId :Uuid, foundAge :Age);
  place @1 PlaceParams -> PlaceResults;
  resolve @2 ResolveParams -> ResolveResults;
  pass @3 (handle :Handle, holder :TextHolder) -> (handle :Handle, said :Text);
}
