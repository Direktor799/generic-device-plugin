use std::{collections::HashMap, time::Duration};

use generic_device_plugin::{
    ContainerAllocateResponse, ContainerPreferredAllocationResponse, Device, DeviceSpec,
    GenericDevicePlugin, GenericDevicePluginServer,
};
use tokio::{
    signal::unix::{signal, SignalKind},
    spawn,
};
use tonic::Status;
use tracing::info;

static DEVICE_PLUGIN_PATH: &str = "/var/lib/kubelet/device-plugins/";
static DEVICE_PLUGIN_SOCK: &str = "mock.sock";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let server = GenericDevicePluginServer::<MockDevicePlugin>::new(
        DEVICE_PLUGIN_PATH.into(),
        DEVICE_PLUGIN_SOCK.to_string(),
    );

    spawn(server.run());

    // k8s is terminating this pod...
    signal(SignalKind::terminate()).unwrap().recv().await;
    info!("SIGTERM received, exiting...");

    Ok(())
}

#[derive(Default)]
pub struct MockDevicePlugin {}

#[async_trait::async_trait]
impl GenericDevicePlugin for MockDevicePlugin {
    const PRE_START_REQUIRED: bool = false;
    const GET_PREFERRED_ALLOCATION_AVAILABLE: bool = false;
    const RESOURCE_NAME: &'static str = "mock.org/mock";
    const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(5);

    async fn get_devices() -> Result<Vec<Device>, Status> {
        let devices = std::fs::read_dir("/dev")
            .map_err(|e| Status::unavailable(e.to_string()))?
            .into_iter()
            .filter_map(|x| {
                let id = x.ok().and_then(|x| x.file_name().into_string().ok())?;
                if id.starts_with("mock") {
                    Some(Device {
                        id,
                        health: String::from("Healthy"),
                        topology: None,
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(devices)
    }

    async fn container_allocate(
        device_ids: Vec<String>,
    ) -> Result<ContainerAllocateResponse, Status> {
        info!("got request to allocate: {device_ids:?}");

        let devices = device_ids
            .into_iter()
            .map(|did| DeviceSpec {
                container_path: format!("/dev/{did}"),
                host_path: format!("/dev/{did}"),
                permissions: String::from("r"),
            })
            .collect();

        Ok(ContainerAllocateResponse {
            envs: HashMap::new(),
            mounts: vec![],
            devices,
            annotations: HashMap::new(),
            cdi_devices: vec![],
        })
    }

    async fn get_container_preferred_allocation(
        _available_device_ids: Vec<String>,
        _must_include_device_ids: Vec<String>,
        _allocation_size: i32,
    ) -> Result<ContainerPreferredAllocationResponse, Status> {
        unimplemented!("PRE_START_REQUIRED = false")
    }

    async fn pre_start_container(_device_ids: Vec<String>) -> Result<(), Status> {
        unimplemented!("GET_PREFERRED_ALLOCATION_AVAILABLE = false")
    }
}
