@0xd7e863a77bb1f889;

using External = import "external.capnp";

# A newtype carrying an annotation whose type is declared in another file.
# A field declared with it carries the annotation too, so code generated for
# that field names `external_capnp::opts`, even in a file that imports only
# this one.
type Tagged = Data $External.annot(field = "tagged");
