# Copyright (c) 2026 the Cap'n Proto authors and contributors
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


# A small generic protocol, compiled to Rust code and also dumped as a CodeGeneratorRequest, for
# the schema loader's `load_compiled` / downcast tests (schema_loader.rs). Each test loads it into
# a fresh loader, so it is kept small.

@0xa93fd7dbdcb1bb42;

enum Kind {
  ping @0;
  pong @1;
}

struct Message(T) {
  id @0 :UInt32;
  payload @1 :T;
  meta :group {
    # A group of a generic struct: it takes its parent's bindings.
    tag @2 :Text;
    kind @3 :Kind;
  }
  body :union {
    none @4 :Void;
    other @5 :T;
  }
}

struct Request(T) {
  body @0 :T;
}

struct Reply(T) {
  body @0 :T;
  ok @1 :Bool;
}

struct Phantom(T) {
  # `T` is bound but never used.
  value @0 :UInt32;
}

struct Outer(T) {
  struct Inner(U) {
    # Nested generic scopes: binds both `T` and `U`.
    t @0 :T;
    u @1 :U;
  }
  inner @0 :Inner(Data);
}

struct Chain {
  # Recursive.
  label @0 :Text;
  next @1 :Chain;
}

struct Envelope {
  single @0 :Message(Request(Text));
  messages @1 :List(Message(Request(Text)));
  phantom @2 :Phantom(Text);
  kinds @3 :List(Kind);
  nested @4 :Outer(Text).Inner(Data);
  chain @5 :Chain;
}

struct UsesEnvelope {
  # Loaded on its own after `load_compiled::<Envelope>()`, which supplies what it refers to.
  envelope @0 :Envelope;
  replies @1 :List(Message(Reply(Text)));
}
