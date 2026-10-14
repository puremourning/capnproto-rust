@0xa640df91b30b9495;

# The payload schema. No Rust code is generated from this file: `build.rs` only embeds its
# `CodeGeneratorRequest`, which `dynamic-dump write` loads at run time to build a sample message.

struct SensorReport {
  station @0 :Text;
  takenAt @1 :UInt64;
  status @2 :Status;
  readings @3 :List(Reading);

  location :group {
    lat @4 :Float64;
    lon @5 :Float64;
  }

  tags @6 :List(Text);

  enum Status {
    ok @0;
    degraded @1;
    offline @2;
  }

  struct Reading {
    sensor @0 :Text;
    union {
      celsius @1 :Float32;
      humidity @2 :Float32;
      raw @3 :Data;
    }
  }
}
