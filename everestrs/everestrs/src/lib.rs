mod schema;

use argh::FromArgs;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("missing argument to command call: '{0}'")]
    MissingArgument(&'static str),
    #[error("invalid argument to command call: '{0}'")]
    InvalidArgument(&'static str),
    #[error("internal Error")]
    Internal,
}

pub type Result<T> = ::std::result::Result<T, Error>;

#[cxx::bridge]
mod ffi {
    // NOCOM(#sirver): This is misnamed at this point, also not really useful.
    struct CommandMeta {
        implementation_id: String,
        name: String,
    }

    extern "Rust" {
        type Runtime;
        fn handle_command(self: &Runtime, meta: &CommandMeta, json: JsonBlob) -> JsonBlob;
        fn handle_variable(self: &Runtime, meta: &CommandMeta, json: JsonBlob);
        fn on_ready(&self);
    }

    struct JsonBlob {
        data: Vec<u8>,
    }

    unsafe extern "C++" {
        include!("everestrs_sys/everestrs_sys.hpp");

        type Module;
        fn create_module(module_id: &str, prefix: &str, conf: &str) -> UniquePtr<Module>;

        /// Connects to the message broker and launches the main everest thread to push work
        /// forward. Returns the module manifest.
        fn initialize(self: &Module) -> JsonBlob;

        /// Returns the interface definition.
        fn get_interface(self: &Module, interface_name: &str) -> JsonBlob;

        /// Registers the callback of the `GenericModule` to be called and calls
        /// `Everest::Module::signal_ready`.
        fn signal_ready(self: &Module, rt: &Runtime);

        /// Informs the runtime that we implement the command described in `meta` and registers the
        /// `handle_command` method from the `GenericModule` as the handler.
        fn provide_command(self: &Module, rt: &Runtime, meta: &CommandMeta);

        /// Call the command described by 'meta' with the given 'args'. Returns the return value.
        fn call_command(
            self: &Module,
            implementation_id: &str,
            name: &str,
            args: JsonBlob,
        ) -> JsonBlob;
        /// Informs the runtime that we want to receive the variable described in `meta` and registers the
        /// `handle_variable` method from the `GenericModule` as the handler.
        fn subscribe_variable(self: &Module, rt: &Runtime, meta: &CommandMeta);

        /// Publishes the given `blob` under the `implementation_id` and `name`.
        fn publish_variable(self: &Module, implementation_id: &str, name: &str, blob: JsonBlob);
    }
}

impl ffi::JsonBlob {
    fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    fn deserialize<T: DeserializeOwned>(self) -> T {
        // TODO(hrapp): Error handling
        serde_json::from_slice(self.as_bytes()).unwrap()
    }

    fn from_vec(data: Vec<u8>) -> Self {
        Self { data }
    }
}

/// The cpp_module is for Rust an opaque type - so Rust can't tell if it is safe
/// to be accessed from multiple threads. We know that the c++ runtime is meant
/// to be used concurrently.
unsafe impl Sync for ffi::Module {}
unsafe impl Send for ffi::Module {}

/// Arguments for an EVerest node.
#[derive(FromArgs, Debug)]
struct Args {
    /// prefix of installation.
    #[argh(option)]
    #[allow(unused)]
    pub prefix: PathBuf,

    /// configuration yml that we are running.
    #[argh(option)]
    #[allow(unused)]
    pub conf: PathBuf,

    /// module name for us.
    #[argh(option)]
    pub module: String,
}

/// Implements the handling of commands & variables, but has no specific information about the
/// details of the current module, i.e. it deals with JSON blobs and strings as command names. Code
/// generation is used to build the concrete, strongly typed abstractions that are then used by
/// final implementors.
pub trait Subscriber: Sync + Send {
    /// Handler for the command `name` on `implementation_id` with the given `parameters`. The return value
    /// will be returned as the result of the call.
    fn handle_command(
        &self,
        implementation_id: &str,
        name: &str,
        parameters: HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value>;

    /// Handler for the variable `name` on `implementation_id` with the given `value`.
    fn handle_variable(
        &self,
        implementation_id: &str,
        name: &str,
        value: serde_json::Value,
    ) -> Result<()>;

    fn on_ready(&self) {}
}

/// The [Runtime] is the central piece of the bridge between c++ and Rust. We
/// have to ensure that the `cpp_module` never outlives the [Runtime] object.
/// This means that the [Runtime] **must** take ownership of `cpp_module`.
///
/// The `Subscriber` is not owned by the [Runtime] - in fact in derived user
/// code the `Subscriber` might take ownership of the [Runtime] - the weak
/// ownership hence is necessary to break possible ownership cycles.
pub struct Runtime {
    cpp_module: cxx::UniquePtr<ffi::Module>,
    sub_impl: Option<Weak<dyn Subscriber>>,
}

impl Runtime {
    fn get_sub(&self) -> Arc<dyn Subscriber> {
        self.sub_impl.as_ref().unwrap().upgrade().unwrap()
    }

    fn on_ready(&self) {
        self.get_sub().on_ready();
    }

    fn handle_command(&self, meta: &ffi::CommandMeta, json: ffi::JsonBlob) -> ffi::JsonBlob {
        let blob = self
            .get_sub()
            .handle_command(&meta.implementation_id, &meta.name, json.deserialize())
            .unwrap();
        ffi::JsonBlob::from_vec(serde_json::to_vec(&blob).unwrap())
    }

    fn handle_variable(&self, meta: &ffi::CommandMeta, json: ffi::JsonBlob) {
        self.get_sub()
            .handle_variable(&meta.implementation_id, &meta.name, json.deserialize())
            .unwrap();
    }

    pub fn publish_variable<T: serde::Serialize>(
        &self,
        impl_id: &str,
        var_name: &str,
        message: &T,
    ) {
        let blob = ffi::JsonBlob::from_vec(
            serde_json::to_vec(&message).expect("Serialization of data cannot fail."),
        );
        (self.cpp_module)
            .as_ref()
            .unwrap()
            .publish_variable(impl_id, var_name, blob);
    }

    pub fn call_command<T: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        impl_id: &str,
        name: &str,
        args: &T,
    ) -> R {
        let blob = ffi::JsonBlob::from_vec(
            serde_json::to_vec(args).expect("Serialization of data cannot fail."),
        );
        let return_value = (self.cpp_module)
            .as_ref()
            .unwrap()
            .call_command(impl_id, name, blob);
        serde_json::from_slice(&return_value.data).unwrap()
    }

    // TODO(hrapp): This function could use some error handling.
    pub fn new() -> Self {
        let args: Args = argh::from_env();
        let cpp_module = ffi::create_module(
            &args.module,
            &args.prefix.to_string_lossy(),
            &args.conf.to_string_lossy(),
        );

        Self {
            cpp_module,
            sub_impl: None,
        }
    }

    pub fn set_subscriber(&mut self, sub_impl: Weak<dyn Subscriber>) {
        if self.sub_impl.is_some() {
            return;
        }
        self.sub_impl = Some(sub_impl);
        let manifest_json = self.cpp_module.as_ref().unwrap().initialize();
        let manifest: schema::Manifest = manifest_json.deserialize();

        // Implement all commands for all of our implementations, dispatch everything to the
        // GenericModule.
        for (implementation_id, implementation) in manifest.provides {
            let interface_s = self.cpp_module.get_interface(&implementation.interface);
            let interface: schema::Interface = interface_s.deserialize();
            for (name, _) in interface.cmds {
                let meta = ffi::CommandMeta {
                    implementation_id: implementation_id.clone(),
                    name,
                };

                (self.cpp_module)
                    .as_ref()
                    .unwrap()
                    .provide_command(&&self, &meta);
            }
        }

        // Subscribe to all variables that might be of interest.
        // TODO(sirver): This looks very similar to the block above.
        for (implementation_id, provides) in manifest.requires {
            let interface_s = self.cpp_module.get_interface(&provides.interface);
            let interface: schema::Interface = interface_s.deserialize();
            for (name, _) in interface.vars {
                // NOCOM(#sirver): Look into misc.cpp, create_setup_from_config to get the right
                // connections here.
                let meta = ffi::CommandMeta {
                    implementation_id: implementation_id.clone(),
                    name,
                };

                (self.cpp_module)
                    .as_ref()
                    .unwrap()
                    .subscribe_variable(&self, &meta);
            }
        }

        // Since users can choose to overwrite `on_ready`, we can call signal_ready right away.
        // TODO(sirver): There were some doubts if this strategy is too inflexible, discuss design
        // again.
        (self.cpp_module).as_ref().unwrap().signal_ready(&self);
    }
}
