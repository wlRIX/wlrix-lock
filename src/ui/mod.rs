// SPDX-License-Identifier: GPL-3.0-or-later
//! The lock screen, as a Wayland client.
//!
//! `ext-session-lock-v1` does the locking; this only asks for it and draws. Once the lock is
//! requested the compositor shows nothing but our lock surfaces (or black, where an output has
//! none), routes all input to them, and keeps doing so until we unlock -- or, if this process
//! dies, for good. `wlrix-compositor/src/session_lock.rs` is the other half, and it fails closed.
//!
//! One lock surface per output, created as soon as the lock is requested so the screens show the
//! lock screen rather than a black frame while the compositor confirms. Outputs plugged in while
//! locked get one too.
//!
//! The rules that keep this a locker rather than a picture of one:
//!
//! - **Unlock only after PAM says yes.** There is no other path to `unlock_and_destroy`.
//! - **Refused is fatal.** If the compositor answers `finished` before `locked` -- another locker
//!   holds the session, or it does not allow locking -- this exits non-zero at once. It never
//!   draws anything that would look like a lock screen over an unlocked session.
//! - **Losing the compositor is not an unlock.** The event loop failing ends the process with an
//!   error; the session lock dies with the compositor anyway.

pub mod draw;
pub mod layout;

use std::time::Duration;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_keyboard, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_session_lock, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::calloop::{
        EventLoop, LoopHandle,
        channel::Event as ChannelEvent,
        timer::{TimeoutAction, Timer},
    },
    reexports::calloop_wayland_source::WaylandSource,
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    session_lock::{
        SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface,
        SessionLockSurfaceConfigure,
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{
        wl_keyboard::WlKeyboard, wl_output::WlOutput, wl_pointer::WlPointer, wl_seat::WlSeat,
        wl_shm, wl_surface::WlSurface,
    },
};
use wlrix_bg::decode::Pictures;
use wlrix_ui::canvas::Canvas;
use wlrix_ui::palette::Palette;
use wlrix_ui::text::Fonts;

use crate::auth::{self, Reply};
use crate::avatar::Avatar;
use crate::clock;
use crate::config::Config;
use crate::pam::Failure;
use crate::secret::Secret;
use crate::user::User;
use layout::Layout;

/// How long each half of the caret's blink lasts -- the greeter's value.
const CARET_BLINK: Duration = Duration::from_millis(530);

/// The Linux button code for the left mouse button.
const BTN_LEFT: u32 = 0x110;

/// One output's lock surface.
struct Screen {
    output: WlOutput,
    surface: SessionLockSurface,
    /// The connector name, for `[[background.output]]`.
    name: Option<String>,
    /// Logical size, as configured. Zero until the first configure, before which nothing may be
    /// attached.
    width: u32,
    height: u32,
    /// The output's integer scale; the buffer is `width * scale` by `height * scale`.
    scale: i32,
    /// The wallpaper at buffer size, blurred if asked, rendered once per configure and copied
    /// under every frame.
    background: Vec<u8>,
    dirty: bool,
}

impl Screen {
    fn buffer_size(&self) -> (i32, i32) {
        (
            self.width as i32 * self.scale,
            self.height as i32 * self.scale,
        )
    }
}

/// Where an unlock attempt has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Typing.
    Idle,
    /// A password is with PAM. Typing is held off until it answers, so a keystroke does not
    /// land in a field about to be cleared.
    Checking,
}

/// How the lock request stands with the compositor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockStatus {
    Requested,
    Locked,
}

/// The whole program's state.
pub struct Locker {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    compositor: CompositorState,
    shm: Shm,
    pool: SlotPool,
    lock: SessionLock,
    status: LockStatus,
    /// Whether the outputs known at startup have their surfaces, after which any new output gets
    /// one as it appears.
    ready: bool,
    /// For the keyboard's repeat timer, which is a source on the same loop.
    loop_handle: LoopHandle<'static, Locker>,

    keyboard: Option<WlKeyboard>,
    pointer: Option<WlPointer>,
    /// The lock surface holding keyboard focus, if any.
    focus: Option<WlSurface>,

    config: Config,
    palette: &'static Palette,
    fonts: Fonts,
    pictures: Pictures,
    avatar: Avatar,
    user: User,

    screens: Vec<Screen>,

    password: Secret,
    phase: Phase,
    auth: auth::Handle,
    message: Option<String>,
    caps_lock: bool,
    caret_on: bool,
    time: String,
    date: String,

    /// Set when the process should end: `Ok` after a successful unlock, `Err` for a refused or
    /// lost lock.
    exit: Option<Result<(), String>>,
}

/// Lock the session and run until it is unlocked.
pub fn run(config: Config) -> Result<(), String> {
    let (palette, unknown) = wlrix_ui::palette::resolve(config.appearance.palette.as_deref());
    if let Some(why) = unknown {
        eprintln!("wlrix-lock: {why}; using {}", palette.id);
    }
    let user = User::current()?;
    let fonts = Fonts::load()?;
    let avatar = Avatar::load(&user.avatar_candidates());
    let (auth, replies) = auth::spawn(user.login.clone())?;

    let conn = Connection::connect_to_env()
        .map_err(|err| format!("no Wayland compositor to connect to: {err}"))?;
    let (globals, mut event_queue) =
        registry_queue_init(&conn).map_err(|err| format!("could not read the registry: {err}"))?;
    let qh = event_queue.handle();

    let mut event_loop: EventLoop<Locker> =
        EventLoop::try_new().map_err(|err| format!("could not create the event loop: {err}"))?;
    let loop_handle = event_loop.handle();

    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|err| format!("wl_compositor unavailable: {err}"))?;
    let shm = Shm::bind(&globals, &qh).map_err(|err| format!("wl_shm unavailable: {err}"))?;
    let session_lock_state = SessionLockState::new(&globals, &qh);
    let pool = SlotPool::new(1920 * 1080 * 4, &shm)
        .map_err(|err| format!("could not create a buffer pool: {err}"))?;

    // Requested before anything else is waited on, so the compositor starts hiding the session
    // as early as it can. The surfaces follow once the outputs are known.
    let lock = session_lock_state.lock(&qh).map_err(|_| {
        "the compositor does not offer ext-session-lock-v1, so the session cannot be locked"
            .to_string()
    })?;

    let tm = clock::now();
    let mut locker = Locker {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        compositor,
        shm,
        pool,
        lock,
        status: LockStatus::Requested,
        ready: false,
        loop_handle: loop_handle.clone(),
        keyboard: None,
        pointer: None,
        focus: None,
        time: String::new(),
        date: String::new(),
        config,
        palette,
        fonts,
        pictures: Pictures::new(),
        avatar,
        user,
        screens: Vec::new(),
        password: Secret::new(),
        phase: Phase::Idle,
        auth,
        message: None,
        caps_lock: false,
        caret_on: true,
        exit: None,
    };
    locker.update_clock(&tm);

    // One round trip delivers the outputs and their names and scales, and the compositor's
    // answer if it refused outright.
    event_queue
        .roundtrip(&mut locker)
        .map_err(|err| format!("could not talk to the compositor: {err}"))?;
    if let Some(Err(why)) = locker.exit.take() {
        return Err(why);
    }
    let outputs: Vec<WlOutput> = locker.output_state.outputs().collect();
    for output in outputs {
        locker.add_screen(&qh, output);
    }
    locker.ready = true;

    WaylandSource::new(conn.clone(), event_queue)
        .insert(loop_handle.clone())
        .map_err(|err| format!("could not drive Wayland from the loop: {err}"))?;

    loop_handle
        .insert_source(replies, |event, _, locker| match event {
            ChannelEvent::Msg(reply) => locker.on_reply(reply),
            // The PAM thread cannot end while this side holds its handle, short of a panic in
            // a PAM module. Say so where it can be read; there is no way to unlock now, and
            // pretending otherwise would leave someone typing into a dead field.
            ChannelEvent::Closed => {
                locker.message = Some("Authentication is unavailable".to_string());
                locker.mark_dirty();
            }
        })
        .map_err(|err| format!("could not receive authentication replies: {err}"))?;

    loop_handle
        .insert_source(Timer::from_duration(CARET_BLINK), |_, _, locker| {
            locker.caret_on = !locker.caret_on;
            locker.mark_dirty();
            TimeoutAction::ToDuration(CARET_BLINK)
        })
        .map_err(|err| format!("could not start the caret timer: {err}"))?;

    let first_tick = Duration::from_secs(clock::secs_to_next_minute(&tm));
    loop_handle
        .insert_source(Timer::from_duration(first_tick), |_, _, locker| {
            let tm = clock::now();
            locker.update_clock(&tm);
            locker.mark_dirty();
            TimeoutAction::ToDuration(Duration::from_secs(clock::secs_to_next_minute(&tm)))
        })
        .map_err(|err| format!("could not start the clock: {err}"))?;

    loop {
        event_loop
            .dispatch(Duration::from_secs(1), &mut locker)
            .map_err(|err| format!("event loop failed: {err}"))?;
        locker.draw_dirty();
        match locker.exit.take() {
            None => {}
            Some(Ok(())) => {
                // Make sure the unlock reached the compositor before the connection closes;
                // a process that exited with it still queued would leave the screen locked.
                conn.roundtrip()
                    .map_err(|err| format!("could not confirm the unlock: {err}"))?;
                return Ok(());
            }
            Some(Err(why)) => return Err(why),
        }
    }
}

impl Locker {
    /// Give `output` a lock surface, if it lacks one.
    fn add_screen(&mut self, qh: &QueueHandle<Self>, output: WlOutput) {
        if self.screens.iter().any(|screen| screen.output == output) {
            return;
        }
        let info = self.output_state.info(&output);
        let name = info.as_ref().and_then(|info| info.name.clone());
        let scale = info.map(|info| info.scale_factor).unwrap_or(1).max(1);
        let surface = self.compositor.create_surface(qh);
        let surface = self.lock.create_lock_surface(surface, &output, qh);
        surface.wl_surface().set_buffer_scale(scale);
        self.screens.push(Screen {
            output,
            surface,
            name,
            width: 0,
            height: 0,
            scale,
            background: Vec::new(),
            dirty: false,
        });
    }

    /// Re-read the time and the date.
    fn update_clock(&mut self, tm: &libc::tm) {
        self.time = clock::format(tm, self.config.time_format());
        self.date = match self.config.date_format() {
            Some(format) => clock::format(tm, format),
            None => clock::format(tm, clock::long_date_format()),
        };
    }

    fn mark_dirty(&mut self) {
        for screen in &mut self.screens {
            screen.dirty = true;
        }
    }

    /// Render the wallpaper for screen `index` at its current size, blurred if asked.
    fn render_background(&mut self, index: usize) {
        let screen = &self.screens[index];
        let (width, height) = screen.buffer_size();
        let wallpaper = self.config.background.resolve(screen.name.as_deref());
        let picture = wallpaper
            .image
            .as_deref()
            .and_then(|path| self.pictures.load(path));
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        wlrix_bg::render::render(&mut pixels, width, height, &wallpaper, picture);
        let radius = (self.config.blur as i32 * screen.scale) as usize;
        crate::blur::blur(&mut pixels, width as usize, height as usize, radius);
        self.screens[index].background = pixels;
    }

    /// Paint every screen that needs it.
    fn draw_dirty(&mut self) {
        for index in 0..self.screens.len() {
            if self.screens[index].dirty && self.screens[index].width > 0 {
                self.draw(index);
            }
        }
    }

    /// Paint screen `index`: the wallpaper, then the lock screen over it.
    fn draw(&mut self, index: usize) {
        let (width, height) = self.screens[index].buffer_size();
        if width <= 0 || height <= 0 {
            return;
        }
        if self.screens[index].background.len() != (width * height * 4) as usize {
            self.render_background(index);
        }
        let Ok((buffer, pixels)) =
            self.pool
                .create_buffer(width, height, width * 4, wl_shm::Format::Xrgb8888)
        else {
            return;
        };
        let screen = &mut self.screens[index];
        screen.dirty = false;
        pixels.copy_from_slice(&screen.background);

        let layout = Layout::compute(width, height, screen.scale);
        let focused = self.focus.as_ref() == Some(screen.surface.wl_surface());
        let view = draw::View {
            time: &self.time,
            date: &self.date,
            name: &self.user.display_name,
            typed: self.password.chars(),
            focused,
            caret_on: self.caret_on,
            checking: self.phase == Phase::Checking,
            message: self.message.as_deref(),
            caps_lock: self.caps_lock,
        };
        let mut canvas = Canvas::new(pixels, width, height);
        draw::paint(
            &mut canvas,
            &layout,
            self.palette,
            &mut self.fonts,
            &mut self.avatar,
            &view,
        );

        let surface = self.screens[index].surface.wl_surface();
        surface.damage_buffer(0, 0, width, height);
        if buffer.attach_to(surface).is_ok() {
            surface.commit();
        }
    }

    /// Hand the password to PAM.
    fn submit(&mut self) {
        if self.phase != Phase::Idle || self.status != LockStatus::Locked {
            return;
        }
        // An empty password is sent too: an account with none must still be able to unlock,
        // and whether that is allowed is the PAM stack's decision (`nullok`), not ours.
        let password = self.password.take();
        if self.auth.check(password) {
            self.phase = Phase::Checking;
            self.message = None;
        } else {
            self.message = Some("Authentication is unavailable".to_string());
        }
        self.mark_dirty();
    }

    /// Act on what PAM said.
    fn on_reply(&mut self, reply: Reply) {
        match reply {
            Reply::Notice(text) => self.message = Some(text),
            Reply::Unlocked => {
                self.lock.unlock();
                self.exit = Some(Ok(()));
            }
            Reply::Failed(Failure::Denied) => {
                self.phase = Phase::Idle;
                self.message = Some("Unlocking failed".to_string());
            }
            Reply::Failed(Failure::Error(why)) => {
                eprintln!("wlrix-lock: authentication error: {why}");
                self.phase = Phase::Idle;
                self.message = Some(why);
            }
        }
        self.mark_dirty();
    }

    /// One keystroke, first press or repeat.
    fn on_key(&mut self, event: KeyEvent) {
        if self.phase != Phase::Idle {
            return;
        }
        match event.keysym {
            Keysym::Return | Keysym::KP_Enter => self.submit(),
            Keysym::BackSpace => self.password.pop(),
            Keysym::Escape => self.password.clear(),
            _ => match event.utf8.as_deref() {
                // Ctrl+U arrives as the control character 0x15, which is the terminal's "kill
                // line" and the field's too.
                Some("\u{15}") => self.password.clear(),
                Some(text) if !text.chars().any(char::is_control) => {
                    self.password.push_str(text);
                    // A message about the last attempt is stale once typing resumes.
                    self.message = None;
                }
                _ => {}
            },
        }
        self.caret_on = true;
        self.mark_dirty();
    }
}

impl SessionLockHandler for Locker {
    fn locked(&mut self, _c: &Connection, _qh: &QueueHandle<Self>, _lock: SessionLock) {
        self.status = LockStatus::Locked;
        eprintln!("wlrix-lock: session locked");
    }

    fn finished(&mut self, _c: &Connection, _qh: &QueueHandle<Self>, _lock: SessionLock) {
        self.exit = Some(Err(match self.status {
            LockStatus::Requested => "the compositor refused to lock the session \
                                      (is another locker already running?)"
                .to_string(),
            // The compositor ended a lock it had granted. Nothing here unlocked it, so this is
            // reported as a failure rather than as the success an unlock would be.
            LockStatus::Locked => "the compositor ended the lock".to_string(),
        }));
    }

    fn configure(
        &mut self,
        _c: &Connection,
        _qh: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _serial: u32,
    ) {
        let Some(index) = self
            .screens
            .iter()
            .position(|screen| screen.surface.wl_surface() == surface.wl_surface())
        else {
            return;
        };
        let (width, height) = configure.new_size;
        let screen = &mut self.screens[index];
        if (screen.width, screen.height) != (width, height) {
            screen.width = width;
            screen.height = height;
            screen.background.clear();
            // Opaque: the compositor need draw nothing under a lock surface.
            if let Ok(region) = Region::new(&self.compositor) {
                region.add(0, 0, width as i32, height as i32);
                screen
                    .surface
                    .wl_surface()
                    .set_opaque_region(Some(region.wl_region()));
            }
        }
        // A configure must be answered with a commit of a buffer of that size, now.
        self.draw(index);
    }
}

impl CompositorHandler for Locker {
    fn scale_factor_changed(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        surface: &WlSurface,
        factor: i32,
    ) {
        let Some(index) = self
            .screens
            .iter()
            .position(|screen| screen.surface.wl_surface() == surface)
        else {
            return;
        };
        let factor = factor.max(1);
        if self.screens[index].scale != factor {
            let screen = &mut self.screens[index];
            screen.scale = factor;
            screen.background.clear();
            surface.set_buffer_scale(factor);
            self.draw(index);
        }
    }
    fn transform_changed(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _s: &WlSurface,
        _t: wayland_client::protocol::wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _c: &Connection, _q: &QueueHandle<Self>, _s: &WlSurface, _t: u32) {}
    fn surface_enter(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _s: &WlSurface,
        _o: &WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _s: &WlSurface,
        _o: &WlOutput,
    ) {
    }
}

impl OutputHandler for Locker {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _c: &Connection, qh: &QueueHandle<Self>, output: WlOutput) {
        // A monitor plugged in while locked gets the lock screen, not black. Until the first
        // round trip is over `run` adds the outputs itself, once their names and scales are in.
        if self.ready {
            self.add_screen(qh, output);
        }
    }
    fn update_output(&mut self, _c: &Connection, _q: &QueueHandle<Self>, output: WlOutput) {
        let Some(info) = self.output_state.info(&output) else {
            return;
        };
        if let Some(screen) = self.screens.iter_mut().find(|s| s.output == output)
            && screen.name != info.name
        {
            screen.name = info.name;
            screen.background.clear();
            screen.dirty = true;
        }
    }
    fn output_destroyed(&mut self, _c: &Connection, _q: &QueueHandle<Self>, output: WlOutput) {
        self.screens.retain(|screen| screen.output != output);
    }
}

impl SeatHandler for Locker {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _c: &Connection, _q: &QueueHandle<Self>, _s: WlSeat) {}

    fn new_capability(
        &mut self,
        _c: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            // With key repeat, so a held Backspace clears a mistyped password.
            let keyboard = self.seat_state.get_keyboard_with_repeat(
                qh,
                &seat,
                None,
                self.loop_handle.clone(),
                Box::new(|locker: &mut Locker, _kbd, event| locker.on_key(event)),
            );
            match keyboard {
                Ok(keyboard) => self.keyboard = Some(keyboard),
                Err(err) => eprintln!("wlrix-lock: no keyboard: {err}"),
            }
        }
        if capability == Capability::Pointer
            && self.pointer.is_none()
            && let Ok(pointer) = self.seat_state.get_pointer(qh, &seat)
        {
            self.pointer = Some(pointer);
        }
    }

    fn remove_capability(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _s: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(keyboard) = self.keyboard.take()
        {
            keyboard.release();
        }
        if capability == Capability::Pointer
            && let Some(pointer) = self.pointer.take()
        {
            pointer.release();
        }
    }

    fn remove_seat(&mut self, _c: &Connection, _q: &QueueHandle<Self>, _s: WlSeat) {}
}

impl KeyboardHandler for Locker {
    fn enter(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _kbd: &WlKeyboard,
        surface: &WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
        self.focus = Some(surface.clone());
        self.caret_on = true;
        self.mark_dirty();
    }

    fn leave(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _kbd: &WlKeyboard,
        _surface: &WlSurface,
        _serial: u32,
    ) {
        self.focus = None;
        self.mark_dirty();
    }

    fn press_key(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _kbd: &WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.on_key(event);
    }

    fn release_key(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _kbd: &WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _kbd: &WlKeyboard,
        _serial: u32,
        modifiers: Modifiers,
        _layout: u32,
    ) {
        if self.caps_lock != modifiers.caps_lock {
            self.caps_lock = modifiers.caps_lock;
            self.mark_dirty();
        }
    }
}

impl PointerHandler for Locker {
    fn pointer_frame(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _pointer: &WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let PointerEventKind::Press { button, .. } = event.kind else {
                continue;
            };
            if button != BTN_LEFT {
                continue;
            }
            let Some(screen) = self
                .screens
                .iter()
                .find(|screen| screen.surface.wl_surface() == &event.surface)
            else {
                continue;
            };
            let (width, height) = screen.buffer_size();
            let layout = Layout::compute(width, height, screen.scale);
            let x = (event.position.0 * f64::from(screen.scale)) as i32;
            let y = (event.position.1 * f64::from(screen.scale)) as i32;
            if layout.on_button(x, y) {
                self.submit();
            }
        }
    }
}

impl ShmHandler for Locker {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Locker {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(Locker);
delegate_output!(Locker);
delegate_shm!(Locker);
delegate_seat!(Locker);
delegate_keyboard!(Locker);
delegate_pointer!(Locker);
delegate_session_lock!(Locker);
delegate_registry!(Locker);
