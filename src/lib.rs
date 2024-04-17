use std::{fs, io::ErrorKind, marker::PhantomData, path::PathBuf};

use anyhow::bail;
use notify::{recommended_watcher, RecursiveMode, Watcher};
use tokio::{
    net::{UnixListener, UnixStream},
    spawn,
    sync::watch,
};
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{
    transport::{Endpoint, Server, Uri},
    Request,
};
use tower::service_fn;
use tracing::{error, info, warn};

use self::pb::{
    device_plugin_server::DevicePluginServer, registration_client::RegistrationClient,
    DevicePluginOptions, RegisterRequest,
};
pub use self::{
    pb::{
        CdiDevice, ContainerAllocateResponse, ContainerPreferredAllocationResponse, Device,
        DeviceSpec, Mount, NumaNode, TopologyInfo,
    },
    service::GenericDevicePlugin,
};

mod service;
mod pb {
    tonic::include_proto!("v1beta1");
}

static VERSION: &str = "v1beta1";
static KUBELET_SOCK: &str = "kubelet.sock";

pub struct GenericDevicePluginServer<DP: GenericDevicePlugin> {
    dir_path: PathBuf,
    socket_name: String,
    _phantom: PhantomData<DP>,
}

impl<DP: GenericDevicePlugin> GenericDevicePluginServer<DP> {
    pub fn new(dir_path: PathBuf, socket_name: String) -> Self {
        Self {
            dir_path,
            socket_name,
            _phantom: PhantomData,
        }
    }

    /// 1. clean up & bind socket
    /// 2. watch socket file (kubelet restart)
    /// 3. start device plugin server
    /// 4. register to kubelet
    /// 5. clean up & goto 1 if socket file changed (graceful)
    pub async fn run(self) -> anyhow::Result<()> {
        let socket_path = self.dir_path.join(&self.socket_name);

        loop {
            match std::os::unix::net::UnixStream::connect(&socket_path) {
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) if e.kind() == ErrorKind::ConnectionRefused => {
                    fs::remove_file(&socket_path)?
                }
                Err(e) => bail!("unable to ensure uds is available: {e:?}"),
                Ok(_) => bail!("active unix socket connect exist on {socket_path:?}"),
            }

            let uds = UnixListener::bind(socket_path.clone())?;

            let (tx, mut rx) = watch::channel(());
            let mut watcher = recommended_watcher(move |res| {
                if let Err(e) = res {
                    error!("failed to watch device plugin socket: {e}")
                }
                let _ = tx.send(());
            })?;

            watcher.watch(&socket_path, RecursiveMode::NonRecursive)?;

            let handle = spawn(
                Server::builder()
                    .add_service(DevicePluginServer::new(DP::default()))
                    .serve_with_incoming_shutdown(UnixListenerStream::new(uds), async move {
                        let _ = rx.changed().await;
                        warn!("socket file changed, restarting server...")
                    }),
            );
            info!("plugin server started on {socket_path:?}!");

            self.register().await?;
            info!("plugin registered!");

            let _ = handle.await;
            let _ = fs::remove_file(&socket_path);
        }
    }

    async fn register(&self) -> anyhow::Result<()> {
        let register_client_socket_path = self.dir_path.join(KUBELET_SOCK);
        RegistrationClient::new(
            Endpoint::try_from("http://[::]:50051")?
                .connect_with_connector(service_fn(move |_: Uri| {
                    UnixStream::connect(register_client_socket_path.clone())
                }))
                .await?,
        )
        .register(Request::new(RegisterRequest {
            endpoint: self.socket_name.clone(),
            resource_name: DP::RESOURCE_NAME.to_string(),
            version: VERSION.to_string(),
            options: Some(DevicePluginOptions {
                pre_start_required: DP::PRE_START_REQUIRED,
                get_preferred_allocation_available: DP::GET_PREFERRED_ALLOCATION_AVAILABLE,
            }),
        }))
        .await?;
        Ok(())
    }
}
