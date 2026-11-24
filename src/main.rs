mod config;
mod state;

use std::sync::Arc;

use winit::{
    application::ApplicationHandler,
    event::{ElementState, KeyEvent, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::PhysicalKey,
    window::{Window, WindowId},
};

use state::State;

#[derive(Default)]
struct App {
    state: Option<State>,
}

impl ApplicationHandler for App {
    // Creates the window and the GPU state once the event loop is running.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("wgpu Part 3: Cloth"))
                .expect("failed to create the window"),
        );

        let state = pollster::block_on(State::new(
            window.clone(),
            event_loop.owned_display_handle(),
        ));
        self.state = Some(state);

        window.request_redraw();
    }

    // Forwards raw mouse motion to the orbit camera.
    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        if let winit::event::DeviceEvent::MouseMotion { delta } = event {
            if let Some(state) = self.state.as_mut() {
                state.mouse_move(delta.0, delta.1);
            }
        }
    }

    // Handles close, resize, key presses and the redraw loop.
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };

        if state.input(&event) {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.resize(size),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => state.key_pressed(code),
            WindowEvent::RedrawRequested => {
                state.update();
                state.render();
                state.window().request_redraw();
            }
            _ => {}
        }
    }
}

// Starts the event loop and runs the app.
fn main() {
    let event_loop = EventLoop::new().expect("failed to create the event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::default();
    event_loop.run_app(&mut app).expect("event loop error");
}
