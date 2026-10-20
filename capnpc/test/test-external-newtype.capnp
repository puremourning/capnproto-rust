@0xc17922e655598894;

using ExternalTypes = import "./external-crate/external-types.capnp";

# Another crate's annotation, reached only through another crate's newtype.
# This file imports external-types.capnp but not external.capnp, yet the code
# generated for `id` names `external_capnp::opts`, the annotation's type.
# The test case is that this builds.
struct UseExternalNewtype {
  id @0 :ExternalTypes.Tagged;
}
