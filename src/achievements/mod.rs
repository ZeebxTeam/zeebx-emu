//! RetroAchievements achievements, through rcheevos' `rc_client`. **Casual mode only:** hardcore
//! requires an emulator approved by RA.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use rcheevos_sys as rc;

pub mod http;

/// The client the window keeps and lends to the session of the open game. Only one: there is only
/// one account.
pub type Shared = Rc<RefCell<Client>>;

pub trait Memory {
    /// Copies `dest.len()` bytes starting at `address`; `false` if the range is not mapped.
    fn read(&self, address: u32, dest: &mut [u8]) -> bool;
}

/// The ranges of RA's address space: the start in RA, the start in the guest and the size.
/// **Provisional: the identity**, until the Zeebo map is agreed with the RA maintainers.
pub const MAP: &[(u32, u32, u64)] = &[(0, 0, 1 << 32)];

/// Where `ra` is in the guest, and how many of the `count` bytes fit before the range ends.
fn translate(ra: u32, count: u32) -> Option<(u32, u32)> {
    let &(start, guest, size) = MAP
        .iter()
        .find(|&&(start, _, size)| start <= ra && u64::from(ra - start) < size)?;
    let offset = ra - start;
    let fits = (size - u64::from(offset)).min(u64::from(count)) as u32;
    Some((guest.checked_add(offset)?, fits))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    /// The form of a POST; `None` is a GET.
    pub body: Option<String>,
}

pub trait Server {
    /// Starts the request. The response goes through the `delivery`, from any thread.
    fn request(&mut self, request: Request, delivery: Delivery);
    fn set_user_agent(&mut self, _user_agent: &str) {}
}

type Callback = unsafe extern "C" fn(*const rc::rc_api_server_response_t, *mut c_void);

struct Arrival {
    callback: Callback,
    data: *mut c_void,
    status: c_int,
    body: Vec<u8>,
}

// SAFETY: the callback is only called on the client's thread, in `Client::deliver_responses`.
unsafe impl Send for Arrival {}

type Queue = Arc<Mutex<Vec<Arrival>>>;

/// Where the response to a request is left. rcheevos expects **exactly one** response per
/// request, and that is why a dropped delivery responds on its own, as a failure.
pub struct Delivery {
    queue: Queue,
    callback: Option<(Callback, *mut c_void)>,
}

// SAFETY: like the `Arrival`.
unsafe impl Send for Delivery {}

impl Delivery {
    pub fn respond(mut self, status: u16, body: Vec<u8>) {
        self.leave(c_int::from(status), body);
    }

    /// The request didn't reach the server: rcheevos tries again when it makes sense.
    pub fn network_error(mut self, message: &str) {
        self.leave(
            rc::RC_API_SERVER_RESPONSE_RETRYABLE_CLIENT_ERROR,
            message.as_bytes().to_vec(),
        );
    }

    fn leave(&mut self, status: c_int, body: Vec<u8>) {
        let Some((callback, data)) = self.callback.take() else {
            return;
        };
        let arrival = Arrival {
            callback,
            data,
            status,
            body,
        };
        // A poisoned lock doesn't spoil the queue, and losing the response would hang the request.
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(arrival);
    }
}

impl Drop for Delivery {
    fn drop(&mut self) {
        self.leave(
            rc::RC_API_SERVER_RESPONSE_CLIENT_ERROR,
            b"the request was dropped without a response".to_vec(),
        );
    }
}

/// What happened, for the interface to show. See [`Client::events`].
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    LoggedIn(Result<(), String>),
    /// `Ok(None)` is a hash RA doesn't know: the game runs without achievements.
    GameLoaded(Result<Option<Game>, String>),
    Unlocked(Achievement),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Achievement {
    pub id: u32,
    pub title: String,
    pub points: u32,
    pub icon: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    pub title: String,
    pub icon: Option<String>,
    pub total: u32,
    pub unlocked: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub name: String,
    /// What is kept in place of the password.
    pub token: String,
    /// The casual mode points.
    pub points: u32,
}

/// RA's hash for a Zeebo game, through `rc_hash`: the main `.mod` together with its `.mif`, which
/// rcheevos finds at `../../mif/<id>.mif`. `None` if the `.mod` is not in that layout.
///
/// From the extracted `.mod`, and not from the package: the hash is the same as the `.zip` it came
/// out of, and so the `.7z`, which rcheevos doesn't read, is identified too.
pub fn game_hash(module: &Path) -> Option<String> {
    // rcheevos opens the path as UTF-8 (on Windows, converted to UTF-16).
    let path = CString::new(module.to_str()?).ok()?;
    let mut hash = [0 as c_char; 33];
    // SAFETY: `hash` has the 33 bytes the header asks for, the path ends in zero, and rcheevos
    // terminates the string.
    unsafe {
        let ok = rc::rc_hash_generate_from_file(hash.as_mut_ptr(), rc::RC_CONSOLE_ZEEBO, path.as_ptr());
        (ok != 0).then(|| text(hash.as_ptr()))
    }
}

/// What the callbacks reach through the client's `userdata`. Accessed **only through a raw
/// pointer**: a call into rcheevos can come back to one of our callbacks, and a live `&mut` on each
/// side would be UB.
struct State {
    server: Box<dyn Server>,
    queue: Queue,
    /// The memory of the frame in progress. See [`Client::with_memory`].
    memory: Option<*const (dyn Memory + 'static)>,
    events: Vec<Event>,
}

/// What the [`crate::session::Session`] keeps of the achievements.
#[derive(Default)]
pub struct SessionAchievements {
    /// The hash of the game the session loaded.
    pub hash: Option<String>,
    pub client: Option<Shared>,
    /// The last 60 Hz frame evaluated, on the virtual clock. `None` re-anchors on the next one.
    pub last_frame: Option<u64>,
}

pub struct Client {
    client: *mut rc::rc_client_t,
    state: *mut State,
}

impl Client {
    /// A logged-out client, in casual mode. `ZEEBX_RA_HOST` swaps the server.
    pub fn new(server: Box<dyn Server>) -> Self {
        let state = Box::into_raw(Box::new(State {
            server,
            queue: Arc::default(),
            memory: None,
            events: Vec::new(),
        }));
        // SAFETY: the callbacks have the header's signatures. The `state` lives until the `Drop`.
        let client = unsafe {
            let client = rc::rc_client_create(Some(read_memory), Some(call_server));
            assert!(!client.is_null(), "rcheevos didn't create the client");
            rc::rc_client_set_userdata(client, state.cast());
            rc::rc_client_set_event_handler(client, Some(on_event));
            rc::rc_client_enable_logging(client, rc::RC_CLIENT_LOG_LEVEL_INFO, Some(log_message));
            // The memory only exists inside the frame.
            rc::rc_client_set_allow_background_memory_reads(client, 0);
            rc::rc_client_set_hardcore_enabled(client, 0);
            client
        };
        let achievements = Self { client, state };
        if let Ok(host) = std::env::var("ZEEBX_RA_HOST")
            && let Ok(host) = CString::new(host.trim())
            && !host.is_empty()
        {
            // SAFETY: rcheevos copies the string.
            unsafe { rc::rc_client_set_host(achievements.client, host.as_ptr()) };
        }
        let user_agent = achievements.user_agent();
        // SAFETY: no call into C in progress; see [`State`].
        unsafe { (*achievements.state).server.set_user_agent(&user_agent) };
        achievements
    }

    /// `Zeebx/<version> (<system>) rcheevos/<version>`: RA requires a user agent per emulator.
    pub fn user_agent(&self) -> String {
        let mut clause = [0 as c_char; 64];
        // SAFETY: the size is the buffer's, and rcheevos terminates the string.
        let clause = unsafe {
            rc::rc_client_get_user_agent_clause(self.client, clause.as_mut_ptr(), 64);
            text(clause.as_ptr())
        };
        let system = std::env::consts::OS;
        format!("Zeebx/{} ({system}) {clause}", env!("CARGO_PKG_VERSION"))
    }

    /// Logs in with username and password. The result arrives as [`Event::LoggedIn`].
    pub fn login_with_password(&mut self, username: &str, password: &str) {
        self.login(username, password, false);
    }

    /// Logs in with the [`User::token`] of a previous login.
    pub fn login_with_token(&mut self, username: &str, token: &str) {
        self.login(username, token, true);
    }

    fn login(&mut self, username: &str, secret: &str, with_token: bool) {
        let (Ok(username), Ok(secret)) = (CString::new(username), CString::new(secret)) else {
            return self.push_event(Event::LoggedIn(Err(
                "nul byte in the username or password".into()
            )));
        };
        let begin = match with_token {
            true => rc::rc_client_begin_login_with_token,
            false => rc::rc_client_begin_login_with_password,
        };
        let null = std::ptr::null_mut();
        // SAFETY: rcheevos copies the strings.
        unsafe {
            begin(
                self.client,
                username.as_ptr(),
                secret.as_ptr(),
                Some(on_login),
                null,
            )
        };
        self.deliver_responses();
    }

    /// Logs out of the account. The loaded game goes with it.
    pub fn logout(&mut self) {
        self.unload_game();
        // SAFETY: client valid until the `Drop`.
        unsafe { rc::rc_client_logout(self.client) };
    }

    pub fn user(&self) -> Option<User> {
        // SAFETY: the pointer is valid until the login changes, and is copied here.
        unsafe {
            let u = rc::rc_client_get_user_info(self.client).as_ref()?;
            Some(User {
                name: text(u.username),
                token: text(u.token),
                points: u.score_casual,
            })
        }
    }

    /// Starts identifying the game by its hash. The [`Event::GameLoaded`] arrives on the first
    /// [`Client::frame`] after the response: rcheevos validates the addresses in it.
    pub fn load_game(&mut self, hash: &str) {
        let Ok(hash) = CString::new(hash) else {
            return;
        };
        let null = std::ptr::null_mut();
        // SAFETY: rcheevos copies the hash.
        unsafe {
            rc::rc_client_begin_load_game(self.client, hash.as_ptr(), Some(on_load), null)
        };
        self.deliver_responses();
    }

    /// First, `rc_client_reset` releases the pending mastery (the last achievement that hasn't had
    /// a frame yet): `rc_client_unload_game` would release it with the game already null, and
    /// rcheevos would crash.
    pub fn unload_game(&mut self) {
        // SAFETY: client valid until the `Drop`.
        unsafe {
            rc::rc_client_reset(self.client);
            rc::rc_client_unload_game(self.client);
        }
    }

    /// Evaluates the achievements of **one** console frame.
    pub fn frame(&mut self, memory: &dyn Memory) {
        self.deliver_responses();
        // SAFETY: client valid until the `Drop`.
        self.with_memory(memory, |client| unsafe {
            rc::rc_client_do_frame(client)
        });
        self.deliver_responses();
    }

    /// No frame advances: only the requests. Without `memory`, `rc_client_idle` is not called: it
    /// may validate the addresses of a freshly loaded game and, without memory, would disable all
    /// of them.
    pub fn idle(&mut self, memory: Option<&dyn Memory>) {
        self.deliver_responses();
        if let Some(memory) = memory {
            // SAFETY: client valid until the `Drop`.
            self.with_memory(memory, |client| unsafe { rc::rc_client_idle(client) });
            self.deliver_responses();
        }
    }

    /// The hit counters, for the save state. Empty without a game.
    pub fn progress(&mut self) -> Vec<u8> {
        // SAFETY: the buffer has the size rcheevos asked for.
        unsafe {
            let size = rc::rc_client_progress_size(self.client);
            let mut data = vec![0u8; size];
            let r = rc::rc_client_serialize_progress_sized(self.client, data.as_mut_ptr(), size);
            if r != rc::RC_OK {
                data.clear();
            }
            data
        }
    }

    /// Goes back to the progress of a save state. Empty resets the counters.
    pub fn restore_progress(&mut self, data: &[u8]) -> bool {
        let pointer = match data.is_empty() {
            true => std::ptr::null(),
            false => data.as_ptr(),
        };
        // SAFETY: rcheevos reads at most `data.len()` bytes; null is its "no progress".
        unsafe {
            rc::rc_client_deserialize_progress_sized(self.client, pointer, data.len())
                == rc::RC_OK
        }
    }

    /// The events since the last call, from oldest to newest.
    pub fn events(&mut self) -> Vec<Event> {
        // SAFETY: no call into C in progress; see [`State`].
        unsafe { std::mem::take(&mut (*self.state).events) }
    }

    fn push_event(&mut self, event: Event) {
        // SAFETY: as above.
        unsafe { (*self.state).events.push(event) };
    }

    /// Runs `f` with the `memory` available to [`read_memory`].
    fn with_memory(&mut self, memory: &dyn Memory, f: impl FnOnce(*mut rc::rc_client_t)) {
        // SAFETY: the erased lifetime doesn't escape: background reads are disabled, and the
        // pointer is taken back before `memory` goes out of scope.
        unsafe {
            let pointer: *const (dyn Memory + '_) = memory;
            (*self.state).memory = Some(std::mem::transmute::<
                *const (dyn Memory + '_),
                *const (dyn Memory + 'static),
            >(pointer));
            f(self.client);
            (*self.state).memory = None;
        }
    }

    /// Hands rcheevos the responses that arrived. A response can generate the next request, which
    /// an immediate server queues during the loop; the cap is only against a broken server.
    fn deliver_responses(&mut self) {
        // SAFETY: the `Arc` is cloned without holding a reference to the `State`.
        let queue = unsafe { Arc::clone(&(*self.state).queue) };
        for _ in 0..1024 {
            let arrivals = std::mem::take(&mut *queue.lock().unwrap_or_else(|e| e.into_inner()));
            if arrivals.is_empty() {
                return;
            }
            for arrival in arrivals {
                let response = rc::rc_api_server_response_t {
                    body: arrival.body.as_ptr().cast(),
                    body_length: arrival.body.len(),
                    http_status_code: arrival.status,
                };
                // SAFETY: the callback rcheevos gave for this request, called once.
                unsafe { (arrival.callback)(&response, arrival.data) };
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.unload_game();
        // SAFETY: the client dies before the state, and no callback is called afterwards.
        unsafe {
            rc::rc_client_destroy(self.client);
            drop(Box::from_raw(self.state));
        }
    }
}

/// A C string, copied; empty for null.
///
/// # Safety
/// `p` is null or points to a zero-terminated string.
unsafe fn text(p: *const c_char) -> String {
    match p.is_null() {
        true => String::new(),
        // SAFETY: by the contract above.
        false => unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned(),
    }
}

/// Like [`text`], with `None` for null and empty.
///
/// # Safety
/// As in [`text`].
unsafe fn maybe_text(p: *const c_char) -> Option<String> {
    // SAFETY: passed on.
    Some(unsafe { text(p) }).filter(|t| !t.is_empty())
}

/// # Safety
/// `client` is one of ours, and Rust holds no reference to the `State` while C runs.
unsafe fn state_of<'a>(client: *const rc::rc_client_t) -> &'a mut State {
    // SAFETY: by the contract above.
    unsafe { &mut *rc::rc_client_get_userdata(client).cast::<State>() }
}

unsafe extern "C" fn read_memory(
    address: u32,
    dest: *mut u8,
    count: u32,
    client: *mut rc::rc_client_t,
) -> u32 {
    // SAFETY: `client` is ours, and `dest` has `count` bytes.
    unsafe {
        let state = state_of(client);
        let (Some(memory), Some((guest, fits))) = (state.memory, translate(address, count)) else {
            return 0;
        };
        let dest = std::slice::from_raw_parts_mut(dest, fits as usize);
        if (*memory).read(guest, dest) {
            fits
        } else {
            0
        }
    }
}

unsafe extern "C" fn call_server(
    request: *const rc::rc_api_request_t,
    callback: rc::rc_client_server_callback_t,
    data: *mut c_void,
    client: *mut rc::rc_client_t,
) {
    // SAFETY: `client` is ours, and the `request` is valid during the call.
    unsafe {
        let Some(callback) = callback else {
            return;
        };
        let state = state_of(client);
        let request = Request {
            url: text((*request).url),
            body: maybe_text((*request).post_data),
        };
        let queue = Arc::clone(&state.queue);
        let callback = Some((callback, data));
        state.server.request(request, Delivery { queue, callback });
    }
}

unsafe extern "C" fn on_login(
    result: c_int,
    message: *const c_char,
    client: *mut rc::rc_client_t,
    _: *mut c_void,
) {
    // SAFETY: `client` is ours, and the message is valid during the call.
    unsafe {
        let event = match result {
            rc::RC_OK => Ok(()),
            _ => Err(text(message)),
        };
        state_of(client).events.push(Event::LoggedIn(event));
    }
}

unsafe extern "C" fn on_load(
    result: c_int,
    message: *const c_char,
    client: *mut rc::rc_client_t,
    _: *mut c_void,
) {
    // SAFETY: as in `on_login`; the game and the summary are copied here.
    unsafe {
        let event = match result {
            rc::RC_OK => rc::rc_client_get_game_info(client).as_ref().map(|game| {
                let mut summary = rc::rc_client_user_game_summary_t::default();
                rc::rc_client_get_user_game_summary(client, &mut summary);
                Game {
                    title: text(game.title),
                    icon: maybe_text(game.badge_url),
                    total: summary.num_promoted_achievements,
                    unlocked: summary.num_unlocked_achievements,
                }
            }),
            rc::RC_NO_GAME_LOADED => None,
            // Another load took this one's place, and that one is the one that reports.
            rc::RC_ABORTED => return,
            _ => {
                return state_of(client)
                    .events
                    .push(Event::GameLoaded(Err(text(message))));
            }
        };
        state_of(client).events.push(Event::GameLoaded(Ok(event)));
    }
}

unsafe extern "C" fn on_event(event: *const rc::rc_client_event_t, client: *mut rc::rc_client_t) {
    // SAFETY: `client` is ours, and the event and what it points to are valid during the call.
    unsafe {
        let e = &*event;
        let Some(a) = e.achievement.as_ref() else {
            return;
        };
        if e.type_ != rc::RC_CLIENT_EVENT_ACHIEVEMENT_TRIGGERED {
            return;
        }
        let mut url = [0 as c_char; 256];
        let state = rc::RC_CLIENT_ACHIEVEMENT_STATE_UNLOCKED;
        let r = rc::rc_client_achievement_get_image_url(a, state, url.as_mut_ptr(), 256);
        state_of(client).events.push(Event::Unlocked(Achievement {
            id: a.id,
            title: text(a.title),
            points: a.points,
            icon: (r == rc::RC_OK).then(|| text(url.as_ptr())),
        }));
    }
}

unsafe extern "C" fn log_message(message: *const c_char, _: *const rc::rc_client_t) {
    // SAFETY: the message is valid during the call.
    let message = unsafe { text(message) };
    crate::logging!(crate::logging::Level::Info, "achievements", "{message}");
}

#[cfg(test)]
mod tests;
