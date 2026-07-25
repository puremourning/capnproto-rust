// Copyright (c) 2024 the Cap'n Proto authors and contributors
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

//! An interface method's parameters and results are ordinary structs, so a `type` newtype used in a
//! method slot must marshal through an actual capability call. `RegistryImpl` reads newtype-typed
//! parameters and writes newtype-typed results; the test drives it through a local client and
//! checks the values round-trip. Access goes through the newtype traits (`vec3`, `order_type`) --
//! the point of the feature -- rather than the per-use-site concrete accessors.

use crate::test_newtype_capnp::{order_type, registry, vec3};
use capnp::Error;
use std::rc::Rc;

// Generic over any use site through the newtype's traits, so these work equally on a plain struct
// field and on a method's parameter/result struct.
fn fill<'a>(v: &mut impl vec3::Builder<'a>, x: f32, y: f32, z: f32) {
    v.set_x(x);
    v.set_y(y);
    v.set_z(z);
}
fn components<'a>(v: &impl vec3::Reader<'a>) -> (f32, f32, f32) {
    (v.get_x(), v.get_y(), v.get_z())
}
fn sum<'a>(v: &impl vec3::Reader<'a>) -> f32 {
    v.get_x() + v.get_y() + v.get_z()
}
fn set_cancel<'a>(ot: &mut impl order_type::Builder<'a>, n: i32) {
    ot.set_cancel(n);
}
fn cancel_of<'a>(ot: &impl order_type::Reader<'a>) -> i32 {
    match ot.which().unwrap() {
        order_type::Which::Cancel(n) => n,
        _ => panic!("expected Cancel arm"),
    }
}

struct RegistryImpl;

impl registry::Server for RegistryImpl {
    async fn lookup(
        self: Rc<Self>,
        params: registry::LookupParams,
        mut results: registry::LookupResults,
    ) -> Result<(), Error> {
        let params = params.get()?;
        let id = params.get_id()?; // uuid::Reader (Data) parameter
        let age = params.get_age(); // age::Reader (u16) parameter
        let mut results = results.get();
        results.set_found_id(id); // echo the Uuid straight back
        results.set_found_age(age + 1); // Age is u16: arithmetic uses the underlying type
        Ok(())
    }

    async fn place(
        self: Rc<Self>,
        params: registry::PlaceParams,
        mut results: registry::PlaceResults,
    ) -> Result<(), Error> {
        let (x, y, z) = {
            let params = params.get()?;
            components(&params.get_spot()) // vec3 group-newtype parameter, read via the trait
        };
        let mut results = results.get();
        fill(&mut results.reborrow().get_echo(), x * 2.0, y * 2.0, z * 2.0); // vec3 result
        set_cancel(&mut results.get_kind(), 99); // order_type union-newtype result
        Ok(())
    }
}

#[test]
fn newtype_in_method_params_and_results() {
    let mut pool = futures::executor::LocalPool::new();
    let client: registry::Client = capnp_rpc::new_client(RegistryImpl);
    pool.run_until(async move {
        // Scalar newtypes (Uuid = Data, Age = UInt16) through an inline parameter/result list.
        let mut request = client.lookup_request();
        {
            let mut params = request.get();
            params.set_id(&[1u8, 2, 3, 4]);
            params.set_age(41);
        }
        let response = request.send().promise.await?;
        let reader = response.get()?;
        assert_eq!(reader.reborrow().get_found_id()?, &[1u8, 2, 3, 4][..]);
        assert_eq!(reader.get_found_age(), 42u16);

        // Group newtype (Vec3) + union newtype (OrderType) via named parameter/result structs.
        let mut request = client.place_request();
        {
            let mut params = request.get();
            fill(&mut params.reborrow().get_spot(), 1.0, 2.0, 3.0);
            params.set_tag(&[0xab_u8]);
        }
        let response = request.send().promise.await?;
        let reader = response.get()?;
        assert_eq!(sum(&reader.reborrow().get_echo()), 12.0);
        assert_eq!(cancel_of(&reader.get_kind()), 99);

        Ok::<(), Error>(())
    })
    .unwrap();
}
