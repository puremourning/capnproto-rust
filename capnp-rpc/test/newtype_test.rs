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

use crate::test_newtype_capnp::{
    endpoint, handle, holder, order_type, registry, text_holder, vec3,
};
use capnp::Error;
use capnp_rpc::{RpcSystem, rpc_twoparty_capnp, twoparty};
use futures::{FutureExt, TryFutureExt};
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

// Calls `lookup` through a capability read out of a newtype, proving the cap table resolved it.
async fn age_via(target: registry::Client, age: u16) -> Result<u16, Error> {
    let mut request = target.lookup_request();
    request.get().set_age(age);
    Ok(request.send().promise.await?.get()?.get_found_age())
}
fn target_of<'a>(ep: &impl endpoint::Reader<'a>) -> Result<registry::Client, Error> {
    ep.get_target()
}

struct RegistryImpl;

struct HolderImpl(&'static str);

impl holder::Server<capnp::text::Owned> for HolderImpl {
    async fn get(
        self: Rc<Self>,
        _: holder::GetParams<capnp::text::Owned>,
        mut results: holder::GetResults<capnp::text::Owned>,
    ) -> Result<(), Error> {
        results.get().set_value(self.0)?;
        Ok(())
    }
}

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
        // vec3 result
        fill(
            &mut results.reborrow().get_echo(),
            x * 2.0,
            y * 2.0,
            z * 2.0,
        );
        set_cancel(&mut results.get_kind(), 99); // order_type union-newtype result
        Ok(())
    }

    async fn resolve(
        self: Rc<Self>,
        params: registry::ResolveParams,
        mut results: registry::ResolveResults,
    ) -> Result<(), Error> {
        // Read the caller's capability out of the request through both the newtype trait and the
        // erased carrier; each must resolve against the request's cap table.
        let (name, via_trait, via_any) = {
            let ep = params.get()?.get_ep();
            let name = ep.get_name()?.to_string()?;
            (name, target_of(&ep)?, ep.as_any().get_target()?)
        };
        if age_via(via_trait, 10).await? != 11 || age_via(via_any, 20).await? != 21 {
            return Err(Error::failed(
                "capability in request newtype misbehaved".into(),
            ));
        }
        // Write a capability into the response through the erased builder carrier.
        let mut ep = results.get().get_ep().as_any();
        ep.set_name(format!("echo:{name}").as_str());
        ep.set_target(capnp_rpc::new_client(RegistryImpl));
        Ok(())
    }

    async fn pass(
        self: Rc<Self>,
        params: registry::PassParams,
        mut results: registry::PassResults,
    ) -> Result<(), Error> {
        let (h, holder): (handle::Client, text_holder::Client) = {
            let params = params.get()?;
            (params.get_handle()?, params.get_holder()?)
        };
        if age_via(h.clone(), 1).await? != 2 {
            return Err(Error::failed("interface alias cap misbehaved".into()));
        }
        let said = holder.get_request().send().promise.await?;
        let mut results = results.get();
        results.set_said(said.get()?.get_value()?);
        results.set_handle(h); // echo the caller's cap back through the alias
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

// Capabilities carried inside a group newtype, across a real two-party connection so the caps go
// through the wire-level cap tables (imbued into the request/response readers and builders).
#[test]
fn newtype_carrying_capability_over_rpc() {
    let mut pool = futures::executor::LocalPool::new();
    let (client_writer, server_reader) = async_byte_channel::channel();
    let (server_writer, client_reader) = async_byte_channel::channel();

    let server = std::thread::spawn(move || {
        let network = twoparty::VatNetwork::new(
            server_reader,
            server_writer,
            rpc_twoparty_capnp::Side::Server,
            Default::default(),
        );
        let bootstrap: registry::Client = capnp_rpc::new_client(RegistryImpl);
        let rpc_system = RpcSystem::new(Box::new(network), Some(bootstrap.client));
        futures::executor::block_on(rpc_system).unwrap();
    });

    let network = twoparty::VatNetwork::new(
        client_reader,
        client_writer,
        rpc_twoparty_capnp::Side::Client,
        Default::default(),
    );
    let mut rpc_system = RpcSystem::new(Box::new(network), None);
    let remote: registry::Client = rpc_system.bootstrap(rpc_twoparty_capnp::Side::Server);
    let disconnector = rpc_system.get_disconnector();
    futures::task::LocalSpawnExt::spawn_local(
        &pool.spawner(),
        rpc_system
            .map_err(|e| panic!("rpc system: {e}"))
            .map(|_| ()),
    )
    .unwrap();

    pool.run_until(async move {
        let mut request = remote.resolve_request();
        {
            // Write through the trait on the concrete per-use-site builder.
            let mut ep = request.get().get_ep();
            endpoint::Builder::set_name(&mut ep, "client");
            endpoint::Builder::set_target(&mut ep, capnp_rpc::new_client(RegistryImpl));
        }
        let response = request.send().promise.await?;
        let ep = response.get()?.get_ep();
        assert_eq!(ep.get_name()?.to_str()?, "echo:client");
        assert_eq!(age_via(target_of(&ep)?, 30).await?, 31);
        assert_eq!(age_via(ep.as_any().get_target()?, 40).await?, 41);

        // Transparent interface aliases (plain and branded) as method params/results.
        let mut request = remote.pass_request();
        {
            let mut params = request.get();
            let h: handle::Client = capnp_rpc::new_client(RegistryImpl);
            params.set_handle(h);
            let t: text_holder::Client = capnp_rpc::new_client(HolderImpl("hello"));
            params.set_holder(t);
        }
        let response = request.send().promise.await?;
        let results = response.get()?;
        assert_eq!(results.get_said()?.to_str()?, "hello");
        let back: registry::Client = results.get_handle()?; // same type as handle::Client
        assert_eq!(age_via(back, 50).await?, 51);
        disconnector.await
    })
    .unwrap();
    server.join().unwrap();
}
