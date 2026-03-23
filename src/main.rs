use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    glib, Align, Box as GtkBox, Button, CheckButton, CssProvider, Entry,
    Label, ListBox, ListBoxRow, Orientation, PolicyType, ScrolledWindow,
    SearchBar, SearchEntry, SelectionMode, Separator, ToggleButton,
};
use libadwaita as adw;
use libadwaita::prelude::*;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Todo {
    text: String,
    done: bool,
    project: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Filter {
    All,
    Active,
    Done,
}

// ── Storage ───────────────────────────────────────────────────────────────────

fn todo_file_path() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
                .join(".local/share")
        });
    let dir = base.join("omado");
    fs::create_dir_all(&dir).ok();
    dir.join("todo.txt")
}

fn parse_project(text: &str) -> Option<String> {
    let colon = text.find(':')?;
    let candidate = text[..colon].trim();
    if candidate.is_empty()
        || candidate.contains(' ')
        || !candidate.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some(candidate.to_string())
}

fn load_todos(path: &PathBuf) -> Vec<Todo> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let (done, raw) = if line.starts_with("[x] ") {
                (true, &line[4..])
            } else if line.starts_with("[ ] ") {
                (false, &line[4..])
            } else {
                (false, line)
            };
            let project = parse_project(raw);
            // text stored WITHOUT "project: " prefix — extract task part
            let text = if let Some(ref p) = project {
                let prefix = format!("{}: ", p);
                if raw.starts_with(&prefix) {
                    raw[prefix.len()..].to_string()
                } else {
                    raw.to_string()
                }
            } else {
                raw.to_string()
            };
            Todo { text, done, project }
        })
        .collect()
}

fn save_todos(path: &PathBuf, todos: &[Todo]) {
    let content = todos
        .iter()
        .map(|t| {
            let mark = if t.done { "[x]" } else { "[ ]" };
            let full_text = match &t.project {
                Some(p) => format!("{}: {}", p, t.text),
                None => t.text.clone(),
            };
            format!("{} {}", mark, full_text)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let content = if content.is_empty() { content } else { content + "\n" };
    fs::write(path, content).ok();
}

// ── App State ─────────────────────────────────────────────────────────────────

struct State {
    todos: Vec<Todo>,
    filter: Filter,
    project_filter: Option<String>, // None = all projects
    search: String,
    path: PathBuf,
}

impl State {
    fn new() -> Self {
        let path = todo_file_path();
        let todos = load_todos(&path);
        State {
            todos,
            filter: Filter::All,
            project_filter: None,
            search: String::new(),
            path,
        }
    }

    fn filtered_indices(&self) -> Vec<usize> {
        let q = self.search.to_lowercase();
        self.todos
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                let by_filter = match self.filter {
                    Filter::All => true,
                    Filter::Active => !t.done,
                    Filter::Done => t.done,
                };
                let by_project = match &self.project_filter {
                    None => true,
                    Some(p) => t.project.as_deref() == Some(p.as_str()),
                };
                let by_search = q.is_empty()
                    || t.text.to_lowercase().contains(&q)
                    || t.project.as_deref().unwrap_or("").to_lowercase().contains(&q);
                by_filter && by_project && by_search
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn projects(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for t in &self.todos {
            if let Some(p) = &t.project {
                if seen.insert(p.clone()) {
                    out.push(p.clone());
                }
            }
        }
        out.sort();
        out
    }

    fn save(&self) {
        save_todos(&self.path, &self.todos);
    }
}

type Shared = Rc<RefCell<State>>;

// ── CLI subcommands ───────────────────────────────────────────────────────────

fn cmd_waybar() {
    let path = todo_file_path();
    let todos = load_todos(&path);
    let active: Vec<&Todo> = todos.iter().filter(|t| !t.done).collect();
    let n = active.len();
    if n == 0 {
        println!(r#"{{"text": "", "tooltip": "No active todos", "class": "empty"}}"#);
        return;
    }
    let lines: Vec<String> = active
        .iter()
        .take(10)
        .map(|t| match &t.project {
            Some(p) => format!("● {}: {}", p, t.text),
            None => format!("● {}", t.text),
        })
        .collect();
    let mut tip = lines.join("\n");
    if active.len() > 10 {
        tip += &format!("\n…and {} more", active.len() - 10);
    }
    println!(
        r#"{{"text": "󰄲 {n}", "tooltip": "{tip}", "class": "active"}}"#,
        n = n,
        tip = tip.replace('"', "'").replace('\n', "\\n")
    );
}

fn cmd_add(text: &str) {
    let path = todo_file_path();
    let mut todos = load_todos(&path);
    let project = parse_project(text);
    let stored_text = if let Some(ref p) = project {
        let prefix = format!("{}: ", p);
        if text.starts_with(&prefix) {
            text[prefix.len()..].to_string()
        } else {
            text.to_string()
        }
    } else {
        text.to_string()
    };
    todos.push(Todo { text: stored_text, done: false, project });
    save_todos(&path, &todos);
    println!("Added: {}", text);
}

// ── Tray (StatusNotifierItem) ─────────────────────────────────────────────────

fn cmd_tray() {
    use ksni::menu::StandardItem;

    struct OmadoTray {
        todos: Vec<Todo>,
        path: PathBuf,
    }

    impl OmadoTray {
        fn new() -> Self {
            let path = todo_file_path();
            let todos = load_todos(&path);
            Self { todos, path }
        }
        fn reload(&mut self) {
            self.todos = load_todos(&self.path);
        }
    }

    impl ksni::Tray for OmadoTray {
        fn id(&self) -> String {
            "omado".into()
        }
        fn title(&self) -> String {
            let n = self.todos.iter().filter(|t| !t.done).count();
            if n == 0 { "omado".into() } else { format!("omado ({})", n) }
        }
        fn icon_name(&self) -> String {
            "checkbox-checked-symbolic".into()
        }
        fn status(&self) -> ksni::Status {
            if self.todos.iter().any(|t| !t.done) {
                ksni::Status::Active
            } else {
                ksni::Status::Passive
            }
        }
        fn tool_tip(&self) -> ksni::ToolTip {
            let active: Vec<&Todo> = self.todos.iter().filter(|t| !t.done).collect();
            let desc = if active.is_empty() {
                "No active todos".to_string()
            } else {
                active.iter().take(10)
                    .map(|t| match &t.project {
                        Some(p) => format!("● {}: {}", p, t.text),
                        None => format!("● {}", t.text),
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            ksni::ToolTip {
                icon_name: "checkbox-checked-symbolic".into(),
                icon_pixmap: vec![],
                title: format!("omado — {} active", active.len()),
                description: desc,
            }
        }
        fn activate(&mut self, _x: i32, _y: i32) {
            std::process::Command::new("omado").spawn().ok();
        }
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            let active: Vec<(usize, &Todo)> = self.todos.iter()
                .enumerate()
                .filter(|(_, t)| !t.done)
                .collect();

            let mut items: Vec<ksni::MenuItem<Self>> = Vec::new();

            if active.is_empty() {
                items.push(StandardItem {
                    label: "No active todos".into(),
                    enabled: false,
                    ..Default::default()
                }.into());
            } else {
                for &(idx, todo) in active.iter().take(15) {
                    let label = match &todo.project {
                        Some(p) => format!("[{}]  {}", p, todo.text),
                        None => todo.text.clone(),
                    };
                    items.push(StandardItem {
                        label,
                        activate: Box::new(move |this: &mut OmadoTray| {
                            if let Some(t) = this.todos.get_mut(idx) {
                                t.done = true;
                            }
                            save_todos(&this.path, &this.todos);
                        }),
                        ..Default::default()
                    }.into());
                }
                if active.len() > 15 {
                    items.push(StandardItem {
                        label: format!("…and {} more", active.len() - 15),
                        enabled: false,
                        ..Default::default()
                    }.into());
                }
            }

            items.push(ksni::MenuItem::Separator);
            items.push(StandardItem {
                label: "Open omado".into(),
                icon_name: "view-list-symbolic".into(),
                activate: Box::new(|_| {
                    std::process::Command::new("omado").spawn().ok();
                }),
                ..Default::default()
            }.into());
            items.push(StandardItem {
                label: "Quick add…".into(),
                icon_name: "list-add-symbolic".into(),
                activate: Box::new(|_| {
                    std::process::Command::new("omado")
                        .arg("quick-add")
                        .spawn()
                        .ok();
                }),
                ..Default::default()
            }.into());

            items
        }
    }

    let service = ksni::TrayService::new(OmadoTray::new());
    let handle = service.handle();
    service.spawn();

    loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        handle.update(|tray| tray.reload());
    }
}

// ── CSS ───────────────────────────────────────────────────────────────────────

const APP_CSS: &str = r#"
.todo-row {
    border-radius: 6px;
}
.todo-label {
    font-size: 14px;
}
.todo-label.done {
    opacity: 0.45;
    text-decoration: line-through;
}
.project-tag {
    font-size: 11px;
    font-weight: bold;
    padding: 1px 8px;
    border-radius: 10px;
}
.tag-0 { background: alpha(@accent_bg_color, 0.3); color: @accent_fg_color; }
.tag-1 { background: alpha(@success_bg_color, 0.3); color: @success_fg_color; }
.tag-2 { background: alpha(@warning_bg_color, 0.3); color: @warning_fg_color; }
.tag-3 { background: alpha(@error_bg_color, 0.3); color: @error_fg_color; }
.tag-4 { background: alpha(@purple_3, 0.3); color: @purple_5; }
.filter-bar {
    padding: 6px 12px;
}
.add-bar {
    padding: 8px 12px;
}
"#;

fn load_css() {
    let provider = CssProvider::new();
    provider.load_from_string(APP_CSS);
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn project_tag_class(project: &str) -> &'static str {
    let h: usize = project.bytes().map(|b| b as usize).sum();
    match h % 5 {
        0 => "tag-0",
        1 => "tag-1",
        2 => "tag-2",
        3 => "tag-3",
        _ => "tag-4",
    }
}

// ── UI helpers ────────────────────────────────────────────────────────────────

fn rebuild(
    list: &ListBox,
    state: &Shared,
    proj_btn: &gtk4::MenuButton,
    proj_list: &ListBox,
) {
    // Collect filtered data before releasing borrow
    let (indices, todos, projects, current_proj) = {
        let s = state.borrow();
        let idx = s.filtered_indices();
        let todos: Vec<Todo> = idx.iter().map(|&i| s.todos[i].clone()).collect();
        (idx, todos, s.projects(), s.project_filter.clone())
    };

    // Clear todo list
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    if todos.is_empty() {
        let lbl = Label::new(Some("No todos"));
        lbl.add_css_class("dim-label");
        lbl.set_margin_top(32);
        lbl.set_margin_bottom(32);
        let row = ListBoxRow::new();
        row.set_activatable(false);
        row.set_selectable(false);
        row.set_child(Some(&lbl));
        list.append(&row);
    } else {
        for (&orig_idx, todo) in indices.iter().zip(todos.iter()) {
            let row = make_row(orig_idx, todo, state, list, proj_btn, proj_list);
            list.append(&row);
        }
    }

    // Update project button label
    proj_btn.set_label(match &current_proj {
        None => "All projects".to_string(),
        Some(p) => p.clone(),
    }.as_str());

    // Rebuild project popover list
    while let Some(child) = proj_list.first_child() {
        proj_list.remove(&child);
    }
    for (label_text, filter_val) in
        std::iter::once(("All projects".to_string(), None))
            .chain(projects.into_iter().map(|p| (p.clone(), Some(p))))
    {
        let row = ListBoxRow::new();
        let lbl = Label::new(Some(&label_text));
        lbl.set_halign(Align::Start);
        lbl.set_margin_start(12);
        lbl.set_margin_end(12);
        lbl.set_margin_top(6);
        lbl.set_margin_bottom(6);
        row.set_child(Some(&lbl));

        let state2 = state.clone();
        let list2 = list.clone();
        let proj_btn2 = proj_btn.clone();
        let proj_list2 = proj_list.clone();
        let popover = proj_btn.popover().unwrap();
        row.connect_activate(move |_| {
            state2.borrow_mut().project_filter = filter_val.clone();
            popover.popdown();
            rebuild(&list2, &state2, &proj_btn2, &proj_list2);
        });

        proj_list.append(&row);
    }
}

fn make_row(
    orig_idx: usize,
    todo: &Todo,
    state: &Shared,
    list: &ListBox,
    proj_btn: &gtk4::MenuButton,
    proj_list: &ListBox,
) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_activatable(false);
    row.set_selectable(false);
    row.add_css_class("todo-row");

    let hbox = GtkBox::new(Orientation::Horizontal, 8);
    hbox.set_margin_top(6);
    hbox.set_margin_bottom(6);
    hbox.set_margin_start(8);
    hbox.set_margin_end(4);

    // Checkbox
    let check = CheckButton::new();
    check.set_active(todo.done);
    check.set_valign(Align::Center);
    {
        let state = state.clone();
        let list = list.clone();
        let proj_btn = proj_btn.clone();
        let proj_list = proj_list.clone();
        check.connect_toggled(move |c| {
            {
                let mut s = state.borrow_mut();
                if let Some(t) = s.todos.get_mut(orig_idx) {
                    t.done = c.is_active();
                }
                s.save();
            }
            rebuild(&list, &state, &proj_btn, &proj_list);
        });
    }
    hbox.append(&check);

    // Project tag
    if let Some(ref p) = todo.project {
        let tag = Label::new(Some(p.as_str()));
        tag.add_css_class("project-tag");
        tag.add_css_class(project_tag_class(p));
        tag.set_valign(Align::Center);
        hbox.append(&tag);
    }

    // Task label (expands)
    let lbl = Label::new(Some(&todo.text));
    lbl.add_css_class("todo-label");
    if todo.done {
        lbl.add_css_class("done");
    }
    lbl.set_hexpand(true);
    lbl.set_halign(Align::Start);
    lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    hbox.append(&lbl);

    // Delete button
    let del = Button::new();
    del.set_icon_name("user-trash-symbolic");
    del.add_css_class("flat");
    del.set_tooltip_text(Some("Delete"));
    del.set_valign(Align::Center);
    del.set_opacity(0.5);
    {
        let state = state.clone();
        let list = list.clone();
        let proj_btn = proj_btn.clone();
        let proj_list = proj_list.clone();
        del.connect_clicked(move |_| {
            {
                let mut s = state.borrow_mut();
                if orig_idx < s.todos.len() {
                    s.todos.remove(orig_idx);
                }
                s.save();
            }
            rebuild(&list, &state, &proj_btn, &proj_list);
        });
    }
    hbox.append(&del);

    row.set_child(Some(&hbox));
    row
}

// ── Quick-add dialog ──────────────────────────────────────────────────────────

fn show_quick_add(app: &adw::Application) {
    let win = adw::ApplicationWindow::builder()
        .application(app)
        .title("Quick add")
        .default_width(420)
        .default_height(80)
        .resizable(false)
        .build();

    let entry = Entry::new();
    entry.set_placeholder_text(Some("Add a todo… (project: task)"));
    entry.set_margin_top(16);
    entry.set_margin_bottom(16);
    entry.set_margin_start(16);
    entry.set_margin_end(16);

    {
        let win = win.clone();
        entry.connect_activate(move |e| {
            let text = e.text().to_string();
            if !text.trim().is_empty() {
                cmd_add(text.trim());
            }
            win.close();
        });
    }

    // Escape closes
    let key_ctrl = gtk4::EventControllerKey::new();
    {
        let win = win.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            if key == gtk4::gdk::Key::Escape {
                win.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    win.add_controller(key_ctrl);
    win.set_content(Some(&entry));
    win.present();
}

// ── Main window ───────────────────────────────────────────────────────────────

fn build_ui(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("omado")
        .default_width(500)
        .default_height(640)
        .build();

    load_css();
    let state: Shared = Rc::new(RefCell::new(State::new()));

    // ── Toolbar view ──────────────────────────────────────────────────────────
    let toolbar_view = adw::ToolbarView::new();

    // Header bar
    let header = adw::HeaderBar::new();

    let search_toggle = gtk4::ToggleButton::new();
    search_toggle.set_icon_name("system-search-symbolic");
    search_toggle.set_tooltip_text(Some("Search (Ctrl+F)"));
    header.pack_end(&search_toggle);

    toolbar_view.add_top_bar(&header);

    // ── Content box ───────────────────────────────────────────────────────────
    let vbox = GtkBox::new(Orientation::Vertical, 0);

    // Search bar
    let search_bar = SearchBar::builder().search_mode_enabled(false).build();
    let search_entry = SearchEntry::new();
    search_entry.set_hexpand(true);
    search_bar.set_child(Some(&search_entry));
    search_bar.connect_search_mode_enabled_notify(glib::clone!(
        #[weak]
        search_toggle,
        move |bar| search_toggle.set_active(bar.is_search_mode())
    ));
    search_toggle.connect_toggled(glib::clone!(
        #[weak]
        search_bar,
        move |btn| search_bar.set_search_mode(btn.is_active())
    ));
    vbox.append(&search_bar);

    // ── Filter bar ────────────────────────────────────────────────────────────
    let filter_bar = GtkBox::new(Orientation::Horizontal, 6);
    filter_bar.add_css_class("filter-bar");

    let btn_all = ToggleButton::with_label("All");
    btn_all.set_active(true);
    let btn_active = ToggleButton::with_label("Active");
    btn_active.set_group(Some(&btn_all));
    let btn_done = ToggleButton::with_label("Done");
    btn_done.set_group(Some(&btn_all));

    filter_bar.append(&btn_all);
    filter_bar.append(&btn_active);
    filter_bar.append(&btn_done);

    // Spacer
    let spacer = GtkBox::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    filter_bar.append(&spacer);

    // Project dropdown (MenuButton + Popover + ListBox)
    let proj_popover_list = ListBox::new();
    proj_popover_list.set_selection_mode(SelectionMode::None);
    proj_popover_list.add_css_class("boxed-list");

    let proj_popover = gtk4::Popover::new();
    let scroll_proj = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .min_content_height(40)
        .max_content_height(260)
        .child(&proj_popover_list)
        .build();
    proj_popover.set_child(Some(&scroll_proj));

    let proj_btn = gtk4::MenuButton::new();
    proj_btn.set_label("All projects");
    proj_btn.add_css_class("flat");
    proj_btn.set_popover(Some(&proj_popover));
    filter_bar.append(&proj_btn);

    vbox.append(&filter_bar);
    vbox.append(&Separator::new(Orientation::Horizontal));

    // ── Todo list ─────────────────────────────────────────────────────────────
    let list = ListBox::new();
    list.set_selection_mode(SelectionMode::None);
    list.add_css_class("boxed-list");
    list.set_margin_top(10);
    list.set_margin_bottom(10);
    list.set_margin_start(12);
    list.set_margin_end(12);

    let scroll = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .vexpand(true)
        .child(&list)
        .build();
    vbox.append(&scroll);

    vbox.append(&Separator::new(Orientation::Horizontal));

    // ── Add bar ───────────────────────────────────────────────────────────────
    let add_bar = GtkBox::new(Orientation::Horizontal, 8);
    add_bar.add_css_class("add-bar");

    let add_entry = Entry::new();
    add_entry.set_hexpand(true);
    add_entry.set_placeholder_text(Some("Add a todo… (use project: task for projects)"));

    let add_btn = Button::new();
    add_btn.set_icon_name("list-add-symbolic");
    add_btn.add_css_class("suggested-action");
    add_btn.set_tooltip_text(Some("Add (Enter)"));
    add_btn.set_valign(Align::Center);

    add_bar.append(&add_entry);
    add_bar.append(&add_btn);
    vbox.append(&add_bar);

    toolbar_view.set_content(Some(&vbox));
    window.set_content(Some(&toolbar_view));

    // ── Initial populate ──────────────────────────────────────────────────────
    rebuild(&list, &state, &proj_btn, &proj_popover_list);

    // ── Wire filter buttons ───────────────────────────────────────────────────
    {
        let state = state.clone();
        let list = list.clone();
        let proj_btn = proj_btn.clone();
        let proj_popover_list = proj_popover_list.clone();
        btn_all.connect_toggled(move |btn| {
            if btn.is_active() {
                state.borrow_mut().filter = Filter::All;
                rebuild(&list, &state, &proj_btn, &proj_popover_list);
            }
        });
    }
    {
        let state = state.clone();
        let list = list.clone();
        let proj_btn = proj_btn.clone();
        let proj_popover_list = proj_popover_list.clone();
        btn_active.connect_toggled(move |btn| {
            if btn.is_active() {
                state.borrow_mut().filter = Filter::Active;
                rebuild(&list, &state, &proj_btn, &proj_popover_list);
            }
        });
    }
    {
        let state = state.clone();
        let list = list.clone();
        let proj_btn = proj_btn.clone();
        let proj_popover_list = proj_popover_list.clone();
        btn_done.connect_toggled(move |btn| {
            if btn.is_active() {
                state.borrow_mut().filter = Filter::Done;
                rebuild(&list, &state, &proj_btn, &proj_popover_list);
            }
        });
    }

    // ── Wire search ───────────────────────────────────────────────────────────
    {
        let state = state.clone();
        let list = list.clone();
        let proj_btn = proj_btn.clone();
        let proj_popover_list = proj_popover_list.clone();
        search_entry.connect_search_changed(move |e| {
            state.borrow_mut().search = e.text().to_string();
            rebuild(&list, &state, &proj_btn, &proj_popover_list);
        });
    }

    // ── Wire add ──────────────────────────────────────────────────────────────
    let do_add = {
        let state = state.clone();
        let list = list.clone();
        let proj_btn = proj_btn.clone();
        let proj_popover_list = proj_popover_list.clone();
        let add_entry = add_entry.clone();
        move || {
            let text = add_entry.text().to_string();
            let text = text.trim();
            if text.is_empty() {
                return;
            }
            let project = parse_project(text);
            let stored = if let Some(ref p) = project {
                let prefix = format!("{}: ", p);
                if text.starts_with(&prefix) {
                    text[prefix.len()..].to_string()
                } else {
                    text.to_string()
                }
            } else {
                text.to_string()
            };
            {
                let mut s = state.borrow_mut();
                s.todos.push(Todo { text: stored, done: false, project });
                s.save();
            }
            add_entry.set_text("");
            rebuild(&list, &state, &proj_btn, &proj_popover_list);
        }
    };

    add_entry.connect_activate({
        let do_add = do_add.clone();
        move |_| do_add()
    });
    add_btn.connect_clicked(move |_| do_add());

    // ── Keyboard shortcuts ────────────────────────────────────────────────────
    let key_ctrl = gtk4::EventControllerKey::new();
    {
        let search_toggle = search_toggle.clone();
        let add_entry = add_entry.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, mods| {
            use gtk4::gdk::Key;
            use gtk4::gdk::ModifierType;
            if mods.contains(ModifierType::CONTROL_MASK) && key == Key::f {
                search_toggle.set_active(!search_toggle.is_active());
                return glib::Propagation::Stop;
            }
            if key == Key::Escape {
                add_entry.set_text("");
                search_toggle.set_active(false);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    window.add_controller(key_ctrl);

    window.present();
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() > 1 {
        match args[1].as_str() {
            "waybar" => {
                cmd_waybar();
                return;
            }
            "tray" => {
                cmd_tray();
                return;
            }
            "add" if args.len() > 2 => {
                cmd_add(&args[2..].join(" "));
                return;
            }
            "quick-add" => {
                // Quick-add mode: tiny floating window for waybar right-click
                adw::init().expect("libadwaita init failed");
                let app = adw::Application::builder()
                    .application_id("dev.omarchy.omado.quickadd")
                    .build();
                app.connect_activate(|app| show_quick_add(app));
                std::process::exit(app.run().into());
            }
            "help" | "--help" | "-h" => {
                println!("omado — todo manager for Omarchy\n");
                println!("USAGE:");
                println!("  omado                   Open the GUI");
                println!("  omado add \"text\"        Add a todo from the command line");
                println!("  omado waybar            Print waybar JSON (active todo count)");
                println!("  omado quick-add         Open the floating quick-add dialog");
                println!("  omado tray              Run as a system tray icon (StatusNotifierItem)");
                println!("\nIn the GUI:");
                println!("  Enter        Add todo (from add bar)");
                println!("  Ctrl+F       Toggle search");
                println!("  Escape       Clear search / cancel");
                return;
            }
            _ => {}
        }
    }

    adw::init().expect("libadwaita init failed");
    let app = adw::Application::builder()
        .application_id("dev.omarchy.omado")
        .build();
    app.connect_activate(build_ui);
    std::process::exit(app.run().into());
}
