use crate::command_processor::*;
use crate::peers_table_view::*;
use crate::settings::Settings;
use crate::tools::*;
use async_tungstenite::tungstenite::http::header::STRICT_TRANSPORT_SECURITY;
use crossbeam_channel::Sender;
use cursive::align::*;
use cursive::event::*;
use cursive::theme::*;
use cursive::traits::*;
use cursive::utils::markup::StyledString;
use cursive::view::SizeConstraint;
use cursive::views::*;
use cursive::Cursive;
use cursive::CursiveRunnable;
use cursive_flexi_logger_view::{CursiveLogWriter, FlexiLoggerView};
// use cursive_multiplex::*;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use chrono::{Datelike, Timelike};

//////////////////////////////////////////////////////////////
///
struct Dirty<T> {
    value: T,
    dirty: bool,
}

impl<T> Dirty<T> {
    pub fn new(value: T) -> Self {
        Self { value, dirty: true }
    }
    pub fn set(&mut self, value: T) {
        self.value = value;
        self.dirty = true;
    }
    pub fn get(&self) -> &T {
        &self.value
    }
    // pub fn get_mut(&mut self) -> &mut T {
    //     &mut self.value
    // }
    pub fn take_dirty(&mut self) -> bool {
        let is_dirty = self.dirty;
        self.dirty = false;
        is_dirty
    }
}

pub type UICallback = Box<dyn Fn(&mut Cursive) + Send>;



static START_TIME: AtomicU64 = AtomicU64::new(0);

struct UIState {
    attachment_state: Dirty<String>,
    public_internet_ready: Dirty<bool>,
    local_network_ready: Dirty<bool>,
    network_started: Dirty<bool>,
    network_down_up: Dirty<(f32, f32)>,
    connection_state: Dirty<ConnectionState>,
    peers_state: Dirty<Vec<json::JsonValue>>,
    node_id: Dirty<String>,
}

impl UIState {
    pub fn new() -> Self {
        Self {
            attachment_state: Dirty::new("Detached".to_owned()),
            public_internet_ready: Dirty::new(false),
            local_network_ready: Dirty::new(false),
            network_started: Dirty::new(false),
            network_down_up: Dirty::new((0.0, 0.0)),
            connection_state: Dirty::new(ConnectionState::Disconnected),
            peers_state: Dirty::new(Vec::new()),
            node_id: Dirty::new("".to_owned()),
        }
    }
}

//#[derive(Error, Debug)]
//#[error("???")]
//struct UIError;

pub struct UIInner {
    ui_state: UIState,
    log_colors: HashMap<Level, cursive::theme::Color>,
    cmdproc: Option<CommandProcessor>,
    cmd_history: VecDeque<String>,
    cmd_history_position: usize,
    cmd_history_max_size: usize,
    connection_dialog_state: Option<ConnectionState>,
}

pub struct UI {
    siv: CursiveRunnable,
    inner: Arc<Mutex<UIInner>>,
}

#[derive(Error, Debug)]
pub enum DumbError {
    // #[error("{0}")]
    // Message(String),
}

impl UI {
    /////////////////////////////////////////////////////////////////////////////////////
    // Private functions
    fn command_processor(s: &mut Cursive) -> CommandProcessor {
        let inner = Self::inner(s);
        inner.cmdproc.as_ref().unwrap().clone()
    }

    fn inner(s: &mut Cursive) -> MutexGuard<'_, UIInner> {
        s.user_data::<Arc<Mutex<UIInner>>>().unwrap().lock()
    }
    fn inner_mut(s: &mut Cursive) -> MutexGuard<'_, UIInner> {
        s.user_data::<Arc<Mutex<UIInner>>>().unwrap().lock()
    }

    fn setup_colors(siv: &mut CursiveRunnable, inner: &mut UIInner, settings: &Settings) {
        // Make colors
        let mut theme = cursive::theme::load_default();
        theme.shadow = settings.interface.theme.shadow;
        theme.borders = BorderStyle::from(&settings.interface.theme.borders);
        theme.palette.set_color(
            "background",
            Color::parse(settings.interface.theme.colors.background.as_str()).unwrap(),
        );
        theme.palette.set_color(
            "shadow",
            Color::parse(settings.interface.theme.colors.shadow.as_str()).unwrap(),
        );
        theme.palette.set_color(
            "view",
            Color::parse(settings.interface.theme.colors.view.as_str()).unwrap(),
        );
        theme.palette.set_color(
            "primary",
            Color::parse(settings.interface.theme.colors.primary.as_str()).unwrap(),
        );
        theme.palette.set_color(
            "secondary",
            Color::parse(settings.interface.theme.colors.secondary.as_str()).unwrap(),
        );
        theme.palette.set_color(
            "tertiary",
            Color::parse(settings.interface.theme.colors.tertiary.as_str()).unwrap(),
        );
        theme.palette.set_color(
            "title_primary",
            Color::parse(settings.interface.theme.colors.title_primary.as_str()).unwrap(),
        );
        theme.palette.set_color(
            "title_secondary",
            Color::parse(settings.interface.theme.colors.title_secondary.as_str()).unwrap(),
        );
        theme.palette.set_color(
            "highlight",
            Color::parse(settings.interface.theme.colors.highlight.as_str()).unwrap(),
        );
        theme.palette.set_color(
            "highlight_inactive",
            Color::parse(settings.interface.theme.colors.highlight_inactive.as_str()).unwrap(),
        );
        theme.palette.set_color(
            "highlight_text",
            Color::parse(settings.interface.theme.colors.highlight_text.as_str()).unwrap(),
        );
        siv.set_theme(theme);

        // Make log colors
        let mut colors = HashMap::<Level, cursive::theme::Color>::new();
        colors.insert(
            Level::Trace,
            Color::parse(settings.interface.theme.log_colors.trace.as_str()).unwrap(),
        );
        colors.insert(
            Level::Debug,
            Color::parse(settings.interface.theme.log_colors.debug.as_str()).unwrap(),
        );
        colors.insert(
            Level::Info,
            Color::parse(settings.interface.theme.log_colors.info.as_str()).unwrap(),
        );
        colors.insert(
            Level::Warn,
            Color::parse(settings.interface.theme.log_colors.warn.as_str()).unwrap(),
        );
        colors.insert(
            Level::Error,
            Color::parse(settings.interface.theme.log_colors.error.as_str()).unwrap(),
        );
        inner.log_colors = colors;
    }
    fn setup_quit_handler(siv: &mut Cursive) {
        siv.clear_global_callbacks(cursive::event::Event::CtrlChar('c'));

        siv.set_on_pre_event(cursive::event::Event::CtrlChar('c'), UI::quit_handler);
        siv.set_global_callback(cursive::event::Event::Key(Key::Esc), UI::quit_handler);
    }

    fn quit_handler(siv: &mut Cursive) {
        siv.add_layer(
            Dialog::text("Do you want to exit?")
                .button("Yes", |s| s.quit())
                .button("No", |s| {
                    s.pop_layer();
                    UI::setup_quit_handler(s);
                }),
        );
        siv.set_on_pre_event(cursive::event::Event::CtrlChar('c'), |s| {
            s.quit();
        });
        siv.set_global_callback(cursive::event::Event::Key(Key::Esc), |s| {
            s.pop_layer();
            UI::setup_quit_handler(s);
        });
    }
    fn clear_handler(siv: &mut Cursive) {
        cursive_flexi_logger_view::clear_log();
        UI::update_cb(siv);
    }
    fn node_events_panel(s: &mut Cursive) -> ViewRef<Panel<ScrollView<FlexiLoggerView>>> {
        s.find_name("node-events-panel").unwrap()
    }
    fn command_line(s: &mut Cursive) -> ViewRef<EditView> {
        s.find_name("command-line").unwrap()
    }
    fn button_attach(s: &mut Cursive) -> ViewRef<Button> {
        s.find_name("button-attach").unwrap()
    }
    fn status_bar(s: &mut Cursive) -> ViewRef<TextView> {
        s.find_name("status-bar").unwrap()
    }
    fn peers(s: &mut Cursive) -> ViewRef<PeersTableView> {
        s.find_name("peers").unwrap()
    }
    fn render_attachment_state(inner: &mut UIInner) -> String {
        let att = match inner.ui_state.attachment_state.get().as_str() {
            "Detached" => "[----]",
            "Attaching" => "[/   ]",
            "AttachedWeak" => "[|   ]",
            "AttachedGood" => "[||  ]",
            "AttachedStrong" => "[||| ]",
            "FullyAttached" => "[||||]",
            "OverAttached" => "[++++]",
            "Detaching" => "[////]",
            _ => "[????]",
        };
        let pi = if *inner.ui_state.public_internet_ready.get() {
            "+P"
        } else {
            "-p"
        };
        let ln = if *inner.ui_state.local_network_ready.get() {
            "+L"
        } else {
            "-l"
        };
        format!("{}{}{}", att, pi, ln)
    }
    fn render_network_status(inner: &mut UIInner) -> String {
        match inner.ui_state.network_started.get() {
            false => "Down: ----KB/s Up: ----KB/s".to_owned(),
            true => {
                let (d, u) = inner.ui_state.network_down_up.get();
                format!("Down: {:.2}KB/s Up: {:.2}KB/s", d, u)
            }
        }
    }
    fn render_button_attach<'a>(inner: &mut UIInner) -> (&'a str, bool) {
        if let ConnectionState::Connected(_, _) = inner.ui_state.connection_state.get() {
            match inner.ui_state.attachment_state.get().as_str() {
                "Detached" => ("Attach", true),
                "Attaching" => ("Detach", true),
                "AttachedWeak" => ("Detach", true),
                "AttachedGood" => ("Detach", true),
                "AttachedStrong" => ("Detach", true),
                "FullyAttached" => ("Detach", true),
                "OverAttached" => ("Detach", true),
                "Detaching" => ("Detach", false),
                _ => ("???", false),
            }
        } else {
            (" ---- ", false)
        }
    }

    fn on_command_line_edit(s: &mut Cursive, text: &str, _pos: usize) {
        let mut inner = Self::inner_mut(s);

        // save edited command to newest history slot
        let hlen = inner.cmd_history.len();
        inner.cmd_history_position = hlen - 1;
        inner.cmd_history[hlen - 1] = text.to_owned();
    }

    fn enable_command_ui(s: &mut Cursive, enabled: bool) {
        Self::command_line(s).set_enabled(enabled);
        Self::button_attach(s).set_enabled(enabled);
    }

    fn display_string_dialog_cb(
        s: &mut Cursive,
        title: String,
        contents: String,
        close_cb: UICallback,
    ) {
        // Creates a dialog around some text with a single button
        let close_cb = Rc::new(close_cb);
        let close_cb2 = close_cb.clone();
        s.add_layer(
            Dialog::around(TextView::new(contents).scrollable())
                .title(title)
                .button("Close", move |s| {
                    s.pop_layer();
                    close_cb(s);
                }),
        );
        s.set_global_callback(cursive::event::Event::Key(Key::Esc), move |s| {
            s.set_global_callback(cursive::event::Event::Key(Key::Esc), UI::quit_handler);
            s.pop_layer();
            close_cb2(s);
        });
    }

    fn run_command(s: &mut Cursive, text: &str) -> Result<(), String> {
        // disable ui
        Self::enable_command_ui(s, false);

        // run command
        s.set_global_callback(cursive::event::Event::Key(Key::Esc), |s| {
            let cmdproc = Self::command_processor(s);
            cmdproc.cancel_command();
        });

        let cmdproc = Self::command_processor(s);
        cmdproc.run_command(
            text,
            Box::new(|s| {
                s.set_global_callback(cursive::event::Event::Key(Key::Esc), UI::quit_handler);
                Self::enable_command_ui(s, true);
            }),
        )
    }

    fn on_command_line_entered(s: &mut Cursive, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        // run command

        cursive_flexi_logger_view::parse_lines_to_log(
            ColorStyle::primary().into(),
            format!("> {} {}", UI::cli_ts(Self::get_start_time()) , text),
        );
        match Self::run_command(s, text) {
            Ok(_) => {}
            Err(e) => {
                let color = *Self::inner_mut(s).log_colors.get(&Level::Error).unwrap();
                cursive_flexi_logger_view::parse_lines_to_log(
                    color.into(),
                    format!(" {} Error: {}", UI::cli_ts(Self::get_start_time()), e)
                );
            }
        }
        // save to history unless it's a duplicate
        {
            let mut inner = Self::inner_mut(s);

            let hlen = inner.cmd_history.len();
            inner.cmd_history[hlen - 1] = text.to_owned();

            if hlen >= 2 && inner.cmd_history[hlen - 1] == inner.cmd_history[hlen - 2] {
                inner.cmd_history[hlen - 1] = "".to_string();
            } else {
                if hlen == inner.cmd_history_max_size {
                    inner.cmd_history.pop_front();
                }
                inner.cmd_history.push_back("".to_string());
            }
            let hlen = inner.cmd_history.len();
            inner.cmd_history_position = hlen - 1;
        }

        // Clear the edit field
        let mut cmdline = Self::command_line(s);
        cmdline.set_content("");
    }

    fn on_command_line_history(s: &mut Cursive, dir: bool) {
        let mut cmdline = Self::command_line(s);
        let mut inner = Self::inner_mut(s);
        // if at top of buffer or end of buffer, ignore
        if (!dir && inner.cmd_history_position == 0)
            || (dir && inner.cmd_history_position == (inner.cmd_history.len() - 1))
        {
            return;
        }

        // move the history position
        if dir {
            inner.cmd_history_position += 1;
        } else {
            inner.cmd_history_position -= 1;
        }

        // replace text with current line
        let hlen = inner.cmd_history_position;
        cmdline.set_content(inner.cmd_history[hlen].as_str());
    }

    fn on_button_attach_pressed(s: &mut Cursive) {
        let action: Option<bool> = match Self::inner_mut(s).ui_state.attachment_state.get().as_str()
        {
            "Detached" => Some(true),
            "Attaching" => Some(false),
            "AttachedWeak" => Some(false),
            "AttachedGood" => Some(false),
            "AttachedStrong" => Some(false),
            "FullyAttached" => Some(false),
            "OverAttached" => Some(false),
            "Detaching" => None,
            _ => None,
        };
        let cmdproc = Self::command_processor(s);
        if let Some(a) = action {
            if a {
                cmdproc.attach();
            } else {
                cmdproc.detach();
            }
        }
    }

    fn refresh_button_attach(s: &mut Cursive) {
        let mut button_attach = UI::button_attach(s);
        let mut inner = Self::inner_mut(s);

        let (button_text, button_enable) = UI::render_button_attach(&mut inner);

        button_attach.set_label(button_text);
        button_attach.set_enabled(button_enable);
    }

    fn submit_connection_address(s: &mut Cursive) {
        let edit = s.find_name::<EditView>("connection-address").unwrap();
        let addr = (*edit.get_content()).clone();
        let sa = match addr.parse::<std::net::SocketAddr>() {
            Ok(sa) => Some(sa),
            Err(_) => {
                s.add_layer(Dialog::text("Invalid address").button("Close", |s| {
                    s.pop_layer();
                }));
                return;
            }
        };
        Self::command_processor(s).set_server_address(sa);
        Self::command_processor(s).start_connection();
    }

    fn copy_to_clipboard<S: AsRef<str>>(s: &mut Cursive, text: S) {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            // X11/Wayland/other system copy
            if clipboard.set_text(text.as_ref()).is_ok() {
                let color = *Self::inner_mut(s).log_colors.get(&Level::Info).unwrap();
                cursive_flexi_logger_view::parse_lines_to_log(
                    color.into(),
                    format!(">> {} Copied: {}", UI::cli_ts(Self::get_start_time()), text.as_ref()),
                );
            } else {
                let color = *Self::inner_mut(s).log_colors.get(&Level::Warn).unwrap();
                cursive_flexi_logger_view::parse_lines_to_log(
                    color.into(),
<<<<<<< HEAD
                    format!(">> {} Could not copy to clipboard",  UI::cli_ts(Self::get_start_time())),
=======
                    ">> Could not copy to clipboard".to_string(),
>>>>>>> f59c4509ea7e0c0e8b1088138a6eb5297844b112
                );
            }
        } else {
            // OSC52 clipboard copy for terminals
            if std::io::stdout()
                .write_all(
                    format!(
                        "\x1B]52;c;{}\x07",
                        data_encoding::BASE64.encode(text.as_ref().as_bytes()),
                    )
                    .as_bytes(),
                )
                .is_ok()
                && std::io::stdout().flush().is_ok()
            {
<<<<<<< HEAD
                if std::io::stdout().flush().is_ok() {
                    let color = *Self::inner_mut(s).log_colors.get(&Level::Info).unwrap();
                    cursive_flexi_logger_view::parse_lines_to_log(
                        color.into(),
                        format!(">> {} Copied: {}", UI::cli_ts(Self::get_start_time()), text.as_ref()),
                    );
                }
=======
                let color = *Self::inner_mut(s).log_colors.get(&Level::Info).unwrap();
                cursive_flexi_logger_view::parse_lines_to_log(
                    color.into(),
                    format!(">> Copied: {}", text.as_ref()),
                );
>>>>>>> f59c4509ea7e0c0e8b1088138a6eb5297844b112
            }
        }
    }

    fn on_submit_peers_table_view(s: &mut Cursive, _row: usize, index: usize) {
        let peers_table_view = UI::peers(s);
        let node_id = peers_table_view
            .borrow_item(index)
            .map(|j| j["node_ids"][0].to_string());
        if let Some(node_id) = node_id {
            Self::copy_to_clipboard(s, node_id);
        }
    }

    fn on_focus_peers_table_view<T>(ptv: &mut ResizedView<T>) -> EventResult {
        ptv.set_height(SizeConstraint::Full);
        EventResult::Ignored
    }

    fn on_focus_lost_peers_table_view<T>(ptv: &mut ResizedView<T>) -> EventResult {
        ptv.set_height(SizeConstraint::AtLeast(8));
        EventResult::Ignored
    }

    fn show_connection_dialog(s: &mut Cursive, state: ConnectionState) -> bool {
        let mut inner = Self::inner_mut(s);

        let mut show: bool = false;
        let mut hide: bool = false;
        let mut reset: bool = false;
        match state {
            ConnectionState::Disconnected => {
                if inner.connection_dialog_state.is_none()
                    || inner
                        .connection_dialog_state
                        .as_ref()
                        .unwrap()
                        .is_connected()
                {
                    show = true;
                } else if inner
                    .connection_dialog_state
                    .as_ref()
                    .unwrap()
                    .is_retrying()
                {
                    reset = true;
                }
            }
            ConnectionState::Connected(_, _) => {
                if inner.connection_dialog_state.is_some()
                    && !inner
                        .connection_dialog_state
                        .as_ref()
                        .unwrap()
                        .is_connected()
                {
                    hide = true;
                }
            }
            ConnectionState::Retrying(_, _) => {
                if inner.connection_dialog_state.is_none()
                    || inner
                        .connection_dialog_state
                        .as_ref()
                        .unwrap()
                        .is_connected()
                {
                    show = true;
                } else if inner
                    .connection_dialog_state
                    .as_ref()
                    .unwrap()
                    .is_disconnected()
                {
                    reset = true;
                }
            }
        }
        inner.connection_dialog_state = Some(state);
        drop(inner);
        if hide {
            s.pop_layer();
            s.pop_layer();
            return true;
        }
        if show {
            s.add_fullscreen_layer(Layer::with_color(
                ResizedView::with_full_screen(DummyView {}),
                ColorStyle::new(PaletteColor::Background, PaletteColor::Background),
            ));
            s.add_layer(
                Dialog::around(
                    LinearLayout::vertical().child(
                        LinearLayout::horizontal()
                            .child(TextView::new("Address:"))
                            .child(
                                EditView::new()
                                    .on_submit(|s, _| Self::submit_connection_address(s))
                                    .with_name("connection-address")
                                    .fixed_height(1)
                                    .min_width(40),
                            ),
                    ),
                )
                .title("Connect to server")
                .with_name("connection-dialog"),
            );

            return true;
        }
        if reset {
            let mut dlg = s.find_name::<Dialog>("connection-dialog").unwrap();
            dlg.clear_buttons();
            return true;
        }

        false
    }

    fn refresh_connection_dialog(s: &mut Cursive) {
        let new_state = Self::inner(s).ui_state.connection_state.get().clone();

        if !Self::show_connection_dialog(s, new_state.clone()) {
            return;
        }

        match new_state {
            ConnectionState::Disconnected => {
                let addr = match Self::command_processor(s).get_server_address() {
                    None => "".to_owned(),
                    Some(addr) => addr.to_string(),
                };
                debug!("address is {}", addr);
                let mut edit = s.find_name::<EditView>("connection-address").unwrap();
                edit.set_content(addr);
                edit.set_enabled(true);
                let mut dlg = s.find_name::<Dialog>("connection-dialog").unwrap();
                dlg.add_button("Connect", Self::submit_connection_address);
            }
            ConnectionState::Connected(_, _) => {}
            ConnectionState::Retrying(addr, _) => {
                //
                let mut edit = s.find_name::<EditView>("connection-address").unwrap();
                debug!("address is {}", addr);
                edit.set_content(addr.to_string());
                edit.set_enabled(false);
                let mut dlg = s.find_name::<Dialog>("connection-dialog").unwrap();
                dlg.add_button("Cancel", |s| {
                    Self::command_processor(s).cancel_reconnect();
                });
            }
        }
    }

    fn refresh_main_titlebar(s: &mut Cursive) {
        let mut main_window = UI::node_events_panel(s);
        let inner = Self::inner_mut(s);
        main_window.set_title(format!("Node: {}", inner.ui_state.node_id.get()));
    }

    fn refresh_statusbar(s: &mut Cursive) {
        let mut statusbar = UI::status_bar(s);

        let mut inner = Self::inner_mut(s);

        let mut status = StyledString::new();

        match inner.ui_state.connection_state.get() {
            ConnectionState::Disconnected => {
                status.append_styled(
                    "Disconnected ".to_string(),
                    ColorStyle::highlight_inactive(),
                );
                status.append_styled("|", ColorStyle::highlight_inactive());
            }
            ConnectionState::Retrying(addr, _) => {
                status.append_styled(
                    format!("Reconnecting to {} ", addr),
                    ColorStyle::highlight_inactive(),
                );
                status.append_styled("|", ColorStyle::highlight_inactive());
            }
            ConnectionState::Connected(addr, _) => {
                status.append_styled(
                    format!("Connected to {} ", addr),
                    ColorStyle::highlight_inactive(),
                );
                status.append_styled("|", ColorStyle::highlight_inactive());
                // Add attachment state
                status.append_styled(
                    format!(" {} ", UI::render_attachment_state(&mut inner)),
                    ColorStyle::highlight_inactive(),
                );
                status.append_styled("|", ColorStyle::highlight_inactive());
                // Add bandwidth status
                status.append_styled(
                    format!(" {} ", UI::render_network_status(&mut inner)),
                    ColorStyle::highlight_inactive(),
                );
                status.append_styled("|", ColorStyle::highlight_inactive());
                // Add tunnel status
                status.append_styled(" No Tunnels ", ColorStyle::highlight_inactive());
                status.append_styled("|", ColorStyle::highlight_inactive());
            }
        };

        statusbar.set_content(status);
    }

    fn refresh_peers(s: &mut Cursive) {
        let mut peers = UI::peers(s);
        let inner = Self::inner_mut(s);
        let sel_item = peers.item();
        let sel_item_text = peers
            .item()
            .map(|x| peers.borrow_items()[x]["node_ids"][0].clone());

        peers.set_items_stable(inner.ui_state.peers_state.get().clone());

        let mut selected = false;
        if let Some(sel_item_text) = sel_item_text {
            // First select by name
            for n in 0..peers.borrow_items().len() {
                if peers.borrow_items()[n]["node_ids"][0] == sel_item_text {
                    peers.set_selected_item(n);
                    selected = true;
                }
            }
        }
        if !selected {
            if let Some(sel_item) = sel_item {
                peers.set_selected_item(sel_item);
            }
        }
    }

    fn update_cb(s: &mut Cursive) {
        let mut inner = Self::inner_mut(s);

        let mut refresh_statusbar = false;
        let mut refresh_button_attach = false;
        let mut refresh_connection_dialog = false;
        let mut refresh_peers = false;
        let mut refresh_main_titlebar = false;
        if inner.ui_state.attachment_state.take_dirty() {
            refresh_statusbar = true;
            refresh_button_attach = true;
            refresh_peers = true;
        }
        if inner.ui_state.network_started.take_dirty() {
            refresh_statusbar = true;
        }
        if inner.ui_state.network_down_up.take_dirty() {
            refresh_statusbar = true;
        }
        if inner.ui_state.connection_state.take_dirty() {
            refresh_statusbar = true;
            refresh_button_attach = true;
            refresh_connection_dialog = true;
            refresh_peers = true;
        }
        if inner.ui_state.peers_state.take_dirty() {
            refresh_peers = true;
        }
        if inner.ui_state.node_id.take_dirty() {
            refresh_main_titlebar = true;
        }

        drop(inner);

        if refresh_statusbar {
            Self::refresh_statusbar(s);
        }
        if refresh_button_attach {
            Self::refresh_button_attach(s);
        }
        if refresh_connection_dialog {
            Self::refresh_connection_dialog(s);
        }
        if refresh_peers {
            Self::refresh_peers(s);
        }
        if refresh_main_titlebar {
            Self::refresh_main_titlebar(s);
        }
    }



    ////////////////////////////////////////////////////////////////////////////
    // Public functions

    pub fn cli_ts(ts: u64) -> String {
        //let ts = get_timestamp();

        let now = chrono::DateTime::<chrono::Utc>::from(SystemTime::now());
        let date = chrono::DateTime::<chrono::Utc>::from(UNIX_EPOCH + Duration::from_micros(ts));

        let show_year = now.year() != date.year();
        let show_month = show_year || now.month() != date.month();
        let show_date = show_month || now.day() != date.day();

        if show_year || show_month || show_date {
            UI::set_start_time();
        }
        format!("{}{}",
            if show_year || show_month || show_date {
                format!("Day changed: {:04}/{:02}/{:02} \n",now.year(), now.month(), now.day()) 
            } else {
                "".to_owned()
            },
            format!("{:02}:{:02}:{:02}",
                    now.hour(),
                    now.minute(),
                    now.second()
            ))
    }

    pub fn set_start_time() {
        START_TIME.store(get_timestamp(), Ordering::Relaxed)
    }

    pub fn get_start_time() -> u64 {
        START_TIME.load(Ordering::Relaxed)
    }

    pub fn new(node_log_scrollback: usize, settings: &Settings) -> (Self, UISender) {
        cursive_flexi_logger_view::resize(node_log_scrollback);

        UI::set_start_time();
        // Instantiate the cursive runnable
        let runnable = CursiveRunnable::new(
            || -> Result<Box<dyn cursive::backend::Backend>, Box<DumbError>> {
                let backend = cursive::backends::crossterm::Backend::init().unwrap();
                let buffered_backend = cursive_buffered_backend::BufferedBackend::new(backend);
                Ok(Box::new(buffered_backend))
            },
        );

        // Make the callback mechanism easily reachable
        let cb_sink = runnable.cb_sink().clone();

        // Create the UI object
        let mut this = Self {
            siv: runnable,
            inner: Arc::new(Mutex::new(UIInner {
                ui_state: UIState::new(),
                log_colors: Default::default(),
                cmdproc: None,
                cmd_history: {
                    let mut vd = VecDeque::new();
                    vd.push_back("".to_string());
                    vd
                },
                cmd_history_position: 0,
                cmd_history_max_size: settings.interface.command_line.history_size,
                connection_dialog_state: None,
            })),
        };

        let ui_sender = UISender {
            inner: this.inner.clone(),
            cb_sink,
        };

        let mut inner = this.inner.lock();

        // Make the inner object accessible in callbacks easily
        this.siv.set_user_data(this.inner.clone());

        // Create layouts

        let node_events_view = Panel::new(FlexiLoggerView::new_scrollable())
            .title_position(HAlign::Left)
            .title("Node Events")
            .with_name("node-events-panel")
            .full_screen();

        let mut peers_table_view = PeersTableView::new()
            .column(PeerTableColumn::NodeId, "Node Id", |c| c.width(48))
            .column(PeerTableColumn::Address, "Address", |c| c)
            .column(PeerTableColumn::LatencyAvg, "Ping", |c| c.width(8))
            .column(PeerTableColumn::TransferDownAvg, "Down", |c| c.width(8))
            .column(PeerTableColumn::TransferUpAvg, "Up", |c| c.width(8));
        peers_table_view.set_on_submit(UI::on_submit_peers_table_view);
        let peers_table_view = FocusTracker::new(ResizedView::new(
            SizeConstraint::Full,
            SizeConstraint::AtLeast(8),
            peers_table_view.with_name("peers"),
        ))
        .on_focus(UI::on_focus_peers_table_view)
        .on_focus_lost(UI::on_focus_lost_peers_table_view);

        // attempt at using Mux. Mux has bugs, like resizing problems.
        // let mut mux = Mux::new();
        // let node_node_events_view = mux
        //     .add_below(node_events_view, mux.root().build().unwrap())
        //     .unwrap();
        // let node_peers_table_view = mux
        //     .add_below(peers_table_view, node_node_events_view)
        //     .unwrap();
        // mux.set_container_split_ratio(node_peers_table_view, 0.75)
        //     .unwrap();
        // let mut mainlayout = LinearLayout::vertical();
        // mainlayout.add_child(mux);

        // Back to fixed layout
        let mut mainlayout = LinearLayout::vertical();
        mainlayout.add_child(node_events_view);
        mainlayout.add_child(peers_table_view);
        // ^^^ fixed layout

        let mut command = StyledString::new();
        command.append_styled("Command> ", ColorStyle::title_primary());
        //
        mainlayout.add_child(
            LinearLayout::horizontal()
                .child(TextView::new(command))
                .child(
                    EditView::new()
                        .on_submit(UI::on_command_line_entered)
                        .on_edit(UI::on_command_line_edit)
                        .on_up_down(UI::on_command_line_history)
                        .style(ColorStyle::new(
                            PaletteColor::Background,
                            PaletteColor::Secondary,
                        ))
                        .with_name("command-line")
                        .full_screen()
                        .fixed_height(1),
                )
                .child(
                    Button::new("Attach", |s| {
                        UI::on_button_attach_pressed(s);
                    })
                    .with_name("button-attach"),
                ),
        );
        let mut version = StyledString::new();
        version.append_styled(
            concat!(" | veilid-cli v", env!("CARGO_PKG_VERSION")),
            ColorStyle::highlight_inactive(),
        );

        mainlayout.add_child(
            LinearLayout::horizontal()
                .color(Some(ColorStyle::highlight_inactive()))
                .child(
                    TextView::new("")
                        .with_name("status-bar")
                        .full_screen()
                        .fixed_height(1),
                )
                .child(TextView::new(version)),
        );

        this.siv.add_fullscreen_layer(mainlayout);

        UI::setup_colors(&mut this.siv, &mut inner, settings);
        UI::setup_quit_handler(&mut this.siv);
        this.siv
            .set_global_callback(cursive::event::Event::CtrlChar('k'), UI::clear_handler);

        drop(inner);

        (this, ui_sender)
    }
    pub fn cursive_flexi_logger(&self) -> Box<CursiveLogWriter> {
        let mut flv = cursive_flexi_logger_view::cursive_flexi_logger(self.siv.cb_sink().clone());
        flv.set_colors(self.inner.lock().log_colors.clone());
        flv
    }
    pub fn set_command_processor(&mut self, cmdproc: CommandProcessor) {
        let mut inner = self.inner.lock();
        inner.cmdproc = Some(cmdproc);
    }

    // Note: Cursive is not re-entrant, can't borrow_mut self.siv again after this
    pub async fn run_async(&mut self) {
        self.siv.run_async().await;
    }
    // pub fn run(&mut self) {
    //      self.siv.run();
    // }

}

type CallbackSink = Box<dyn FnOnce(&mut Cursive) + 'static + Send>;

#[derive(Clone)]
pub struct UISender {
    inner: Arc<Mutex<UIInner>>,
    cb_sink: Sender<CallbackSink>,
}

impl UISender {
    pub fn display_string_dialog<T: ToString, S: ToString>(
        &self,
        title: T,
        text: S,
        close_cb: UICallback,
    ) {
        let title = title.to_string();
        let text = text.to_string();
        let _ = self.cb_sink.send(Box::new(move |s| {
            UI::display_string_dialog_cb(s, title, text, close_cb)
        }));
    }

    pub fn quit(&self) {
        let _ = self.cb_sink.send(Box::new(|s| {
            s.quit();
        }));
    }

    pub fn send_callback(&self, callback: UICallback) {
        let _ = self.cb_sink.send(Box::new(move |s| callback(s)));
    }
    pub fn set_attachment_state(
        &mut self,
        state: String,
        public_internet_ready: bool,
        local_network_ready: bool,
    ) {
        {
            let mut inner = self.inner.lock();
            inner.ui_state.attachment_state.set(state);
            inner
                .ui_state
                .public_internet_ready
                .set(public_internet_ready);
            inner.ui_state.local_network_ready.set(local_network_ready);
        }

        let _ = self.cb_sink.send(Box::new(UI::update_cb));
    }
    pub fn set_network_status(
        &mut self,
        started: bool,
        bps_down: u64,
        bps_up: u64,
        mut peers: Vec<json::JsonValue>,
    ) {
        {
            let mut inner = self.inner.lock();
            inner.ui_state.network_started.set(started);
            inner.ui_state.network_down_up.set((
                ((bps_down as f64) / 1000.0f64) as f32,
                ((bps_up as f64) / 1000.0f64) as f32,
            ));
            peers.sort_by(|a, b| {
                a["node_ids"][0]
                    .to_string()
                    .cmp(&b["node_ids"][0].to_string())
            });
            inner.ui_state.peers_state.set(peers);
        }
        let _ = self.cb_sink.send(Box::new(UI::update_cb));
    }
    pub fn set_config(&mut self, config: &json::JsonValue) {
        let mut inner = self.inner.lock();

        let node_ids = &config["network"]["routing_table"]["node_id"];

        let mut node_id_str = String::new();
        for l in 0..node_ids.len() {
            let nid = &node_ids[l];
            if !node_id_str.is_empty() {
                node_id_str.push(' ');
            }
            node_id_str.push_str(nid.to_string().as_ref());
        }

        inner.ui_state.node_id.set(node_id_str);
    }
    pub fn set_connection_state(&mut self, state: ConnectionState) {
        {
            let mut inner = self.inner.lock();
            inner.ui_state.connection_state.set(state);
        }
        let _ = self.cb_sink.send(Box::new(UI::update_cb));
    }

    pub fn add_node_event(&self, log_color: Level, event: String) {
        {
            let inner = self.inner.lock();
            let color = *inner.log_colors.get(&log_color).unwrap();
            cursive_flexi_logger_view::parse_lines_to_log(color.into(), format!("{}: {}", UI::cli_ts(UI::get_start_time()), event));
        }
        let _ = self.cb_sink.send(Box::new(UI::update_cb));
    }
}
