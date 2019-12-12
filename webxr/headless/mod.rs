/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::SessionBuilder;
use crate::SwapChains;

use webxr_api::DeviceAPI;
use webxr_api::DiscoveryAPI;
use webxr_api::Error;
use webxr_api::Event;
use webxr_api::EventBuffer;
use webxr_api::Floor;
use webxr_api::Frame;
use webxr_api::FrameUpdateEvent;
use webxr_api::Input;
use webxr_api::InputFrame;
use webxr_api::InputSource;
use webxr_api::MockDeviceInit;
use webxr_api::MockDeviceMsg;
use webxr_api::MockDiscoveryAPI;
use webxr_api::MockInputMsg;
use webxr_api::Native;
use webxr_api::Quitter;
use webxr_api::Receiver;
use webxr_api::Sender;
use webxr_api::Session;
use webxr_api::SessionMode;
use webxr_api::Viewer;
use webxr_api::Views;

use euclid::RigidTransform3D;

use std::sync::{Arc, Mutex};
use std::thread;

use surfman::platform::generic::universal::surface::Surface;

pub struct HeadlessMockDiscovery {}

struct HeadlessDiscovery {
    data: Arc<Mutex<HeadlessDeviceData>>,
    supports_immersive: bool,
}

struct InputInfo {
    source: InputSource,
    active: bool,
    pointer: Option<RigidTransform3D<f32, Input, Native>>,
    grip: Option<RigidTransform3D<f32, Input, Native>>,
}

struct HeadlessDevice {
    data: Arc<Mutex<HeadlessDeviceData>>,
    mode: SessionMode,
}

struct HeadlessDeviceData {
    floor_transform: Option<RigidTransform3D<f32, Native, Floor>>,
    viewer_origin: Option<RigidTransform3D<f32, Viewer, Native>>,
    views: Views,
    needs_view_update: bool,
    needs_floor_update: bool,
    inputs: Vec<InputInfo>,
    events: EventBuffer,
    quitter: Option<Quitter>,
    disconnected: bool,
}

impl MockDiscoveryAPI<SwapChains> for HeadlessMockDiscovery {
    fn simulate_device_connection(
        &mut self,
        init: MockDeviceInit,
        receiver: Receiver<MockDeviceMsg>,
    ) -> Result<Box<dyn DiscoveryAPI<SwapChains>>, Error> {
        let viewer_origin = init.viewer_origin.clone();
        let floor_transform = init.floor_origin.map(|f| f.inverse());
        let views = init.views.clone();
        let data = HeadlessDeviceData {
            floor_transform,
            viewer_origin,
            views,
            needs_view_update: false,
            needs_floor_update: false,
            inputs: vec![],
            events: Default::default(),
            quitter: None,
            disconnected: false,
        };
        let data = Arc::new(Mutex::new(data));
        let data_ = data.clone();

        thread::spawn(move || {
            run_loop(receiver, data_);
        });
        Ok(Box::new(HeadlessDiscovery {
            data,
            supports_immersive: init.supports_immersive,
        }))
    }
}

fn run_loop(receiver: Receiver<MockDeviceMsg>, data: Arc<Mutex<HeadlessDeviceData>>) {
    while let Ok(msg) = receiver.recv() {
        if !data.lock().expect("Mutex poisoned").handle_msg(msg) {
            break;
        }
    }
}

impl DiscoveryAPI<SwapChains> for HeadlessDiscovery {
    fn request_session(&mut self, mode: SessionMode, xr: SessionBuilder) -> Result<Session, Error> {
        if !self.supports_session(mode) {
            return Err(Error::NoMatchingDevice);
        }
        let data = self.data.clone();
        xr.run_on_main_thread(move || Ok(HeadlessDevice { data, mode }))
    }

    fn supports_session(&self, mode: SessionMode) -> bool {
        (!self.data.lock().unwrap().disconnected)
            && (mode == SessionMode::Inline || self.supports_immersive)
    }
}

impl DeviceAPI<Surface> for HeadlessDevice {
    fn floor_transform(&self) -> Option<RigidTransform3D<f32, Native, Floor>> {
        self.data.lock().unwrap().floor_transform.clone()
    }

    fn views(&self) -> Views {
        if self.mode == SessionMode::Inline {
            Views::Inline
        } else {
            self.data.lock().unwrap().views.clone()
        }
    }

    fn wait_for_animation_frame(&mut self) -> Option<Frame> {
        thread::sleep(std::time::Duration::from_millis(20));
        let time_ns = time::precise_time_ns();
        let mut data = self.data.lock().unwrap();
        let transform = data.viewer_origin;
        let inputs = data
            .inputs
            .iter()
            .filter(|i| i.active)
            .map(|i| InputFrame {
                id: i.source.id,
                target_ray_origin: i.pointer,
                grip_origin: i.grip,
                pressed: false,
                squeezed: false,
            })
            .collect();

        let mut events = if data.needs_view_update {
            data.needs_view_update = false;
            vec![FrameUpdateEvent::UpdateViews(self.views())]
        } else {
            vec![]
        };

        if data.needs_floor_update {
            events.push(FrameUpdateEvent::UpdateFloorTransform(
                data.floor_transform.clone(),
            ));
            data.needs_floor_update = false;
        }
        Some(Frame {
            transform,
            inputs,
            events,
            time_ns,
        })
    }

    fn render_animation_frame(&mut self, surface: Surface) -> Surface {
        surface
    }

    fn initial_inputs(&self) -> Vec<InputSource> {
        vec![]
    }

    fn set_event_dest(&mut self, dest: Sender<Event>) {
        self.data.lock().unwrap().events.upgrade(dest)
    }

    fn quit(&mut self) {
        self.data.lock().unwrap().events.callback(Event::SessionEnd);
    }

    fn set_quitter(&mut self, quitter: Quitter) {
        self.data.lock().unwrap().quitter = Some(quitter);
    }

    fn update_clip_planes(&mut self, _: f32, _: f32) {
        // The views are actually set through the test API so this does nothing
        // https://github.com/immersive-web/webxr-test-api/issues/39
    }
}

impl HeadlessMockDiscovery {
    pub fn new() -> HeadlessMockDiscovery {
        HeadlessMockDiscovery {}
    }
}

impl HeadlessDeviceData {
    fn handle_msg(&mut self, msg: MockDeviceMsg) -> bool {
        match msg {
            MockDeviceMsg::SetViewerOrigin(viewer_origin) => {
                self.viewer_origin = viewer_origin;
            }
            MockDeviceMsg::SetFloorOrigin(floor_origin) => {
                self.floor_transform = floor_origin.map(|f| f.inverse());
                self.needs_floor_update = true;
            }
            MockDeviceMsg::SetViews(views) => {
                self.views = views;
                self.needs_view_update = true;
            }
            MockDeviceMsg::Focus => {
                // TODO
            }
            MockDeviceMsg::Blur => {
                // TODO
            }
            MockDeviceMsg::AddInputSource(init) => {
                self.inputs.push(InputInfo {
                    source: init.source,
                    pointer: init.pointer_origin,
                    grip: init.grip_origin,
                    active: true,
                });
                self.events.callback(Event::AddInput(init.source))
            }
            MockDeviceMsg::MessageInputSource(id, msg) => {
                if let Some(ref mut input) = self.inputs.iter_mut().find(|i| i.source.id == id) {
                    match msg {
                        MockInputMsg::SetHandedness(h) => input.source.handedness = h,
                        MockInputMsg::SetTargetRayMode(t) => input.source.target_ray_mode = t,
                        MockInputMsg::SetPointerOrigin(p) => input.pointer = p,
                        MockInputMsg::SetGripOrigin(p) => input.grip = p,
                        MockInputMsg::Disconnect => input.active = false,
                        MockInputMsg::Reconnect => input.active = true,
                    }
                }
            }
            MockDeviceMsg::Disconnect(s) => {
                self.disconnected = true;
                self.quitter.as_ref().map(|q| q.quit());
                // notify the client that we're done disconnecting
                let _ = s.send(());
                return false;
            }
        }
        true
    }
}
