use std::{pin::Pin, time::Duration};

use tokio::{sync::mpsc, time::sleep};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{codegen::tokio_stream::Stream, Request, Response, Status};
use tracing::{error, info};

use super::pb::{device_plugin_server::DevicePlugin, *};

#[async_trait::async_trait]
pub trait GenericDevicePlugin: 'static + Sync + Send + Default {
    const PRE_START_REQUIRED: bool;
    const GET_PREFERRED_ALLOCATION_AVAILABLE: bool;
    const RESOURCE_NAME: &'static str;
    const DEVICE_POLL_INTERVAL: Duration;

    async fn get_devices() -> Result<Vec<Device>, Status>;

    async fn container_allocate(
        device_ids: Vec<String>,
    ) -> Result<ContainerAllocateResponse, Status>;

    async fn get_container_preferred_allocation(
        available_device_ids: Vec<String>,
        must_include_device_ids: Vec<String>,
        allocation_size: i32,
    ) -> Result<ContainerPreferredAllocationResponse, Status>;

    async fn pre_start_container(device_ids: Vec<String>) -> Result<(), Status>;
}

#[async_trait::async_trait]
impl<DP: GenericDevicePlugin> DevicePlugin for DP {
    /// GetDevicePluginOptions returns options to be communicated with Device
    /// Manager
    async fn get_device_plugin_options(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<DevicePluginOptions>, Status> {
        let resp = DevicePluginOptions {
            pre_start_required: DP::PRE_START_REQUIRED,
            get_preferred_allocation_available: DP::GET_PREFERRED_ALLOCATION_AVAILABLE,
        };
        return Ok(Response::new(resp));
    }

    /// Server streaming response type for the ListAndWatch method.
    type ListAndWatchStream =
        Pin<Box<dyn Stream<Item = Result<ListAndWatchResponse, Status>> + Send>>;

    /// ListAndWatch returns a stream of List of Devices
    /// Whenever a Device state change or a Device disappears, ListAndWatch
    /// returns the new list
    async fn list_and_watch(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<Self::ListAndWatchStream>, Status> {
        let (tx, rx) = mpsc::channel(128);
        tokio::spawn(async move {
            let mut prev_devices = Err(Status::unknown(""));
            loop {
                if tx.is_closed() {
                    break;
                }

                let devices_resp = DP::get_devices().await;

                // if error or changed
                if devices_resp.is_err() || devices_resp.as_ref().ok() != prev_devices.as_ref().ok()
                {
                    prev_devices = devices_resp.clone();
                    match tx
                        .send(devices_resp.map(|x| ListAndWatchResponse { devices: x }))
                        .await
                    {
                        Ok(()) => match &prev_devices {
                            Ok(pd) => info!("found {} devices, new device list sent!", pd.len()),
                            Err(e) => error!("failed to get devices: {e}"),
                        },
                        Err(e) => {
                            error!("failed to send new device list: {e}");
                            break;
                        }
                    }
                }
                sleep(DP::DEVICE_POLL_INTERVAL).await;
            }
            info!("list and watch disconnected!");
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    /// GetPreferredAllocation returns a preferred set of devices to allocate
    /// from a list of available ones. The resulting preferred allocation is not
    /// guaranteed to be the allocation ultimately performed by the
    /// devicemanager. It is only designed to help the devicemanager make a more
    /// informed allocation decision when possible.
    async fn get_preferred_allocation(
        &self,
        request: Request<PreferredAllocationRequest>,
    ) -> Result<Response<PreferredAllocationResponse>, Status> {
        let request = request.into_inner();
        let mut container_responses = Vec::with_capacity(request.container_requests.len());
        for req in request.container_requests {
            container_responses.push(
                DP::get_container_preferred_allocation(
                    req.available_device_i_ds,
                    req.must_include_device_i_ds,
                    req.allocation_size,
                )
                .await?,
            );
        }
        return Ok(Response::new(PreferredAllocationResponse {
            container_responses,
        }));
    }

    /// Allocate is called during container creation so that the Device
    /// Plugin can run device specific operations and instruct Kubelet
    /// of the steps to make the Device available in the container
    async fn allocate(
        &self,
        request: Request<AllocateRequest>,
    ) -> Result<Response<AllocateResponse>, Status> {
        let request = request.into_inner();
        let mut container_responses = Vec::with_capacity(request.container_requests.len());
        for req in request.container_requests {
            container_responses.push(DP::container_allocate(req.devices_ids).await?);
        }
        return Ok(Response::new(AllocateResponse {
            container_responses,
        }));
    }

    /// PreStartContainer is called, if indicated by Device Plugin during registeration phase,
    /// before each container start. Device plugin can run device specific operations
    /// such as resetting the device before making devices available to the container
    async fn pre_start_container(
        &self,
        request: Request<PreStartContainerRequest>,
    ) -> Result<Response<PreStartContainerResponse>, Status> {
        DP::pre_start_container(request.into_inner().devices_ids).await?;
        return Ok(Response::new(PreStartContainerResponse {}));
    }
}
