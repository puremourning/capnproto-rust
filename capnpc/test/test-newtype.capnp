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

# Exercises `type` newtypes stamped inline with `@[...]`. Requires a newtype-capable `capnp`
# compiler on PATH (this repo's companion C++ build); an older `capnp` cannot parse it.

@0xf9d8e7c6b5a49382;

using Import = import "test-newtype-import.capnp";

type Vec3 = group {                  # flat group newtype over primitives
  x @0 :Float32;
  y @1 :Float32;
  z @2 :Float32;
}

type Named = group {                 # a Text pointer field alongside a data field
  label @0 :Text;
  count @1 :Int32;
}

struct Coord { lat @0 :Float64; lng @1 :Float64; }

type Boxed = group {                 # struct + list + text pointer fields
  at @0 :Coord;
  tags @1 :List(Int32);
  note @2 :Text;
}

type Status = union {                # union newtype with all-slot arms (void / data / pointer)
  pending @0 :Void;
  code @1 :Int32;
  label @2 :Text;
}

type Price = group {                 # a newtype used as a nested member below
  value @0 :Int64;
  scale @1 :UInt16;
}

type OrderPrices = group {           # nested newtype members -> associated-type getters
  limit @[0-1] :Price;
  stop @[2-3] :Price;
}

type OrderType = union {             # union newtype with a nested-newtype (Price) group arm
  limit @[0-1] :Price;
  market @2 :Void;
  cancel @3 :Int32;
}

type Priced = group {                # a field with an explicit default
  amount @0 :Int64;
  scale @1 :UInt16 = 100;
}

type Uuid = Data;                    # scalar newtype over a pointer type -> alias module
type Age = UInt16;                   # scalar newtype over a value type -> alias module
type Ids = List(UInt64);             # scalar newtype over a List -> pointer alias module

struct Shapes {
  topLeft @[0, 1, 2] :Vec3;          # two Vec3 use sites at distinct offsets
  bottomRight @[3, 4, 5] :Vec3;
  named @[6, 7] :Named;
  boxed @[8, 9, 10] :Boxed;
  status @[11, 12, 13] :Status;
  id @14 :Uuid;                      # scalar newtypes used as ordinary fields
  age @15 :Age;
  prices @[16, 17, 18, 19] :OrderPrices;  # nested newtype (Price members)
  order @[20, 21, 22, 23] :OrderType;     # union newtype with a group arm
  priced @[24, 25] :Priced;               # explicit-default field, complete mapping
  pricedPartial @[26] :Priced;            # incomplete: scale unmapped -> reads its default (100)
  ids @27 :Ids;                           # scalar List newtype used as an ordinary field
}

struct CrossFile {
  # Newtypes imported from test-newtype-import.capnp: their `type` node and template live in the
  # other file, so this exercises the compiler pulling cross-file newtype nodes into the request.
  id @0 :Import.ImportedId;             # scalar pointer newtype -> imported alias module
  age @1 :Import.ImportedAge;           # scalar value newtype -> imported alias module
  corner @[2, 3, 4] :Import.ImportedVec;   # group newtype -> imported wrapper module
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

interface Registry {
  # Newtypes in interface method parameter/result slots. Scalar newtypes may appear directly in an
  # inline parameter list; group/union newtypes come in via the named structs above.
  lookup @0 (id :Uuid, age :Age) -> (foundId :Uuid, foundAge :Age);
  place @1 PlaceParams -> PlaceResults;
}
