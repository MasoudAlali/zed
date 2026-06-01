use anyhow::Result;
use collections::HashSet;
use editor::Editor;
use gpui::{
    actions, div, list, px, Action, AnyElement, App, AsyncWindowContext, ClickEvent, Context,
    Entity, EventEmitter, FocusHandle, Focusable, IntoElement, ListAlignment, ListState,
    ParentElement, Render, SharedString, Styled, Task, WeakEntity, Window,
};
use icons::IconName;
use log;
use node_runtime::NodeRuntime;
use project::{Project, ProjectPath};
use serde::Deserialize;
use std::path::Path;
use text::Point;
use ui::{
    prelude::*, Button, ButtonStyle, Color, Icon, Label, LabelSize, Tab, Tooltip, h_flex, v_flex,
};
use util::{paths::PathStyle, rel_path::RelPath, ResultExt};
use workspace::{
    dock::{DockPosition, Panel, PanelEvent},
    Workspace,
};
use worktree::WorktreeId;

const KNIP_PANEL_KEY: &str = "KnipPanel";

actions!(
    knip_panel,
    [
        /// Toggle the knip panel.
        ToggleFocus,
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<KnipPanel>(window, cx);
        });
    })
    .detach();
}

#[derive(Debug, Clone)]
enum KnipEntry {
    SectionHeader {
        title: SharedString,
        icon: IconName,
        section_index: usize,
        count: usize,
    },
    FileItem {
        path: SharedString,
        worktree_id: Option<WorktreeId>,
        section_index: usize,
    },
    IssueItem {
        file_path: SharedString,
        name: SharedString,
        issue_type: IssueType,
        worktree_id: Option<WorktreeId>,
        line: Option<u32>,
        col: Option<u32>,
        section_index: usize,
    },
}

impl KnipEntry {
    fn section_index(&self) -> usize {
        match self {
            KnipEntry::SectionHeader { section_index, .. } => *section_index,
            KnipEntry::FileItem { section_index, .. } => *section_index,
            KnipEntry::IssueItem { section_index, .. } => *section_index,
        }
    }

    fn is_header(&self) -> bool {
        matches!(self, KnipEntry::SectionHeader { .. })
    }
}

#[derive(Debug, Clone, Copy)]
enum IssueType {
    Dependency,
    DevDependency,
    UnlistedDependency,
    UnlistedBinary,
    Export,
    Type,
    Duplicate,
    Enum,
}

impl IssueType {
    fn label(self) -> &'static str {
        match self {
            IssueType::Dependency => "dependency",
            IssueType::DevDependency => "devDependency",
            IssueType::UnlistedDependency => "unlisted",
            IssueType::UnlistedBinary => "unlisted binary",
            IssueType::Export => "export",
            IssueType::Type => "type",
            IssueType::Duplicate => "duplicate",
            IssueType::Enum => "enum member",
        }
    }

    fn color(self) -> Color {
        match self {
            IssueType::Dependency | IssueType::DevDependency => Color::Error,
            IssueType::UnlistedDependency | IssueType::UnlistedBinary => Color::Warning,
            IssueType::Export | IssueType::Type => Color::Accent,
            IssueType::Duplicate | IssueType::Enum => Color::Muted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RunState {
    Idle,
    Running,
    Done,
    Error,
}

pub struct KnipPanel {
    project: Entity<Project>,
    workspace: WeakEntity<Workspace>,
    node_runtime: NodeRuntime,
    focus_handle: FocusHandle,
    position: DockPosition,
    all_entries: Vec<KnipEntry>,
    visible_entries: Vec<usize>,
    collapsed_sections: HashSet<usize>,
    entry_list: ListState,
    run_state: RunState,
    error_message: Option<String>,
    run_task: Option<Task<()>>,
    pending_navigation: Option<Point>,
    total_issues: usize,
    section_count: usize,
}

impl KnipPanel {
    pub fn new(
        workspace: &mut Workspace,
        _window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        let project = workspace.project().clone();
        let workspace_handle = workspace.weak_handle();
        let node_runtime = workspace.app_state().node_runtime.clone();

        cx.new(|cx| {
            let entry_list = ListState::new(0, ListAlignment::Top, px(1000.));

            Self {
                project,
                workspace: workspace_handle,
                node_runtime,
                focus_handle: cx.focus_handle(),
                position: DockPosition::Left,
                all_entries: Vec::new(),
                visible_entries: Vec::new(),
                collapsed_sections: HashSet::default(),
                entry_list,
                run_state: RunState::Idle,
                error_message: None,
                run_task: None,
                pending_navigation: None,
                total_issues: 0,
                section_count: 0,
            }
        })
    }

    pub fn load(
        workspace: WeakEntity<Workspace>,
        cx: AsyncWindowContext,
    ) -> Task<Result<Entity<Self>>> {
        cx.spawn(async move |cx| {
            workspace.update_in(cx, |workspace, window, cx| Self::new(workspace, window, cx))
        })
    }

    fn rebuild_visible_entries(&mut self) {
        self.visible_entries.clear();
        for (ix, entry) in self.all_entries.iter().enumerate() {
            if entry.is_header() || !self.collapsed_sections.contains(&entry.section_index()) {
                self.visible_entries.push(ix);
            }
        }
        self.entry_list.reset(self.visible_entries.len());
    }

    fn toggle_section(&mut self, section_index: usize, cx: &mut Context<Self>) {
        if self.collapsed_sections.contains(&section_index) {
            self.collapsed_sections.remove(&section_index);
        } else {
            self.collapsed_sections.insert(section_index);
        }
        self.rebuild_visible_entries();
        cx.notify();
    }

    fn run_knip(&mut self, cx: &mut Context<Self>) {
        log::info!("knip: run_knip called, current state: {:?}", self.run_state);

        if self.run_state == RunState::Running {
            return;
        }

        self.run_state = RunState::Running;
        self.error_message = None;
        self.all_entries.clear();
        self.visible_entries.clear();
        self.collapsed_sections.clear();
        self.total_issues = 0;
        self.section_count = 0;
        self.entry_list.reset(0);
        cx.notify();

        let project = self.project.read(cx);
        let worktree = project.visible_worktrees(cx).next();
        let (worktree_abs_path, worktree_id) = match worktree {
            Some(wt) => {
                let wt = wt.read(cx);
                (wt.abs_path().to_path_buf(), Some(wt.id()))
            }
            None => {
                self.run_state = RunState::Error;
                self.error_message = Some("No project open.".into());
                cx.notify();
                return;
            }
        };

        log::info!("knip: running in {}", worktree_abs_path.display());
        let node_runtime = self.node_runtime.clone();

        self.run_task = Some(cx.spawn(async move |this, cx| {
            log::info!("knip: async task started, resolving node runtime...");
            let result = run_knip_process(&worktree_abs_path, &node_runtime).await;

            match &result {
                Ok(report) => log::info!("knip: finished — {} issues", report.issues.len()),
                Err(error) => log::error!("knip: error — {error:#}"),
            }

            this.update(cx, |this, cx| match result {
                Ok(report) => {
                    this.apply_report(report, worktree_id);
                    this.run_state = RunState::Done;
                    // Collapse all sections except the first
                    for i in 1..this.section_count {
                        this.collapsed_sections.insert(i);
                    }
                    this.rebuild_visible_entries();
                    cx.notify();
                }
                Err(error) => {
                    this.run_state = RunState::Error;
                    this.error_message = Some(format!("{error:#}"));
                    cx.notify();
                }
            })
            .log_err();
        }));
    }

    fn apply_report(&mut self, report: KnipReport, worktree_id: Option<WorktreeId>) {
        self.all_entries.clear();
        self.total_issues = 0;
        self.section_count = 0;

        let unused_files: Vec<String> = report
            .issues
            .iter()
            .flat_map(|issue| issue.files.iter().map(|f| f.name.clone()))
            .collect();

        if !unused_files.is_empty() {
            let section_index = self.section_count;
            self.section_count += 1;
            let count = unused_files.len();
            self.total_issues += count;
            self.all_entries.push(KnipEntry::SectionHeader {
                title: "Unused Files".into(),
                count,
                icon: IconName::File,
                section_index,
            });
            for file in &unused_files {
                self.all_entries.push(KnipEntry::FileItem {
                    path: file.clone().into(),
                    worktree_id,
                    section_index,
                });
            }
        }

        struct IssueCategory {
            title: &'static str,
            icon: IconName,
            issue_type: IssueType,
            items: Vec<(String, String, Option<u32>, Option<u32>)>,
        }

        let mut dependencies = Vec::new();
        let mut dev_dependencies = Vec::new();
        let mut unlisted = Vec::new();
        let mut unlisted_binaries = Vec::new();
        let mut exports = Vec::new();
        let mut types = Vec::new();
        let mut duplicates = Vec::new();
        let mut enum_members = Vec::new();

        for issue in &report.issues {
            for item in &issue.dependencies {
                dependencies.push((issue.file.clone(), item.name.clone(), item.line, item.col));
            }
            for item in &issue.dev_dependencies {
                dev_dependencies.push((issue.file.clone(), item.name.clone(), item.line, item.col));
            }
            for item in &issue.unlisted {
                unlisted.push((issue.file.clone(), item.name.clone(), item.line, item.col));
            }
            for item in &issue.binaries {
                unlisted_binaries.push((issue.file.clone(), item.name.clone(), item.line, item.col));
            }
            for item in &issue.exports {
                exports.push((issue.file.clone(), item.name.clone(), item.line, item.col));
            }
            for item in &issue.types {
                types.push((issue.file.clone(), item.name.clone(), item.line, item.col));
            }
            for item in &issue.duplicates {
                duplicates.push((issue.file.clone(), item.name.clone(), item.line, item.col));
            }
            for item in &issue.enum_members {
                enum_members.push((issue.file.clone(), item.name.clone(), item.line, item.col));
            }
        }

        let categories = [
            IssueCategory {
                title: "Unused Dependencies",
                icon: IconName::Library,
                issue_type: IssueType::Dependency,
                items: dependencies,
            },
            IssueCategory {
                title: "Unused Dev Dependencies",
                icon: IconName::Library,
                issue_type: IssueType::DevDependency,
                items: dev_dependencies,
            },
            IssueCategory {
                title: "Unlisted Dependencies",
                icon: IconName::Warning,
                issue_type: IssueType::UnlistedDependency,
                items: unlisted,
            },
            IssueCategory {
                title: "Unlisted Binaries",
                icon: IconName::Warning,
                issue_type: IssueType::UnlistedBinary,
                items: unlisted_binaries,
            },
            IssueCategory {
                title: "Unused Exports",
                icon: IconName::Code,
                issue_type: IssueType::Export,
                items: exports,
            },
            IssueCategory {
                title: "Unused Types",
                icon: IconName::Code,
                issue_type: IssueType::Type,
                items: types,
            },
            IssueCategory {
                title: "Duplicates",
                icon: IconName::Copy,
                issue_type: IssueType::Duplicate,
                items: duplicates,
            },
            IssueCategory {
                title: "Unused Enum Members",
                icon: IconName::Code,
                issue_type: IssueType::Enum,
                items: enum_members,
            },
        ];

        for category in categories {
            if category.items.is_empty() {
                continue;
            }
            let section_index = self.section_count;
            self.section_count += 1;
            let count = category.items.len();
            self.total_issues += count;
            self.all_entries.push(KnipEntry::SectionHeader {
                title: category.title.into(),
                count,
                icon: category.icon,
                section_index,
            });
            for (file_path, name, line, col) in category.items {
                self.all_entries.push(KnipEntry::IssueItem {
                    file_path: file_path.into(),
                    name: name.into(),
                    issue_type: category.issue_type,
                    worktree_id,
                    line,
                    col,
                    section_index,
                });
            }
        }
    }

    fn open_file(
        &mut self,
        path: &str,
        worktree_id: Option<WorktreeId>,
        line: Option<u32>,
        col: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree_id) = worktree_id else {
            return;
        };
        let Ok(rel_path) = RelPath::new(Path::new(path), PathStyle::Posix) else {
            return;
        };
        let project_path: ProjectPath = (worktree_id, rel_path.as_ref()).into();

        let task = self
            .workspace
            .update(cx, |workspace, cx| {
                workspace.open_path(project_path, None, true, window, cx)
            })
            .ok();

        let Some(task) = task else {
            return;
        };

        if let Some(line) = line {
            let row = line.saturating_sub(1);
            let column = col.unwrap_or(1).saturating_sub(1);
            self.pending_navigation = Some(Point::new(row, column));
        }
        task.detach_and_log_err(cx);
        // Schedule a deferred notify so render picks up pending_navigation
        // after the file has opened
        let entity = cx.entity().downgrade();
        window.defer(cx, move |_window, cx| {
            if let Some(this) = entity.upgrade() {
                this.update(cx, |_, cx| cx.notify());
            }
        });
    }

    fn render_entry(
        &self,
        visible_ix: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let entry_ix = *self.visible_entries.get(visible_ix)?;
        let entry = self.all_entries.get(entry_ix)?;

        match entry {
            KnipEntry::SectionHeader {
                title,
                icon,
                section_index,
                count,
            } => {
                let title = title.clone();
                let icon = *icon;
                let section_index = *section_index;
                let count = *count;
                let is_collapsed = self.collapsed_sections.contains(&section_index);
                let chevron = if is_collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                };

                Some(
                    div()
                        .id(("section", visible_ix))
                        .w_full()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.toggle_section(section_index, cx);
                        }))
                        .child(
                            h_flex()
                                .w_full()
                                .px_2()
                                .py_1p5()
                                .gap_1()
                                .bg(cx.theme().colors().surface_background)
                                .border_b_1()
                                .border_color(cx.theme().colors().border_variant)
                                .child(
                                    Icon::new(chevron)
                                        .size(IconSize::Small)
                                        .color(Color::Muted),
                                )
                                .child(
                                    Icon::new(icon).size(IconSize::Small).color(Color::Muted),
                                )
                                .child(
                                    Label::new(title)
                                        .size(LabelSize::Small)
                                        .weight(gpui::FontWeight::SEMIBOLD)
                                        .color(Color::Default),
                                )
                                .child(
                                    Label::new(format!("({count})"))
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                ),
                        )
                        .into_any(),
                )
            }
            KnipEntry::FileItem {
                path, worktree_id, ..
            } => {
                let path = path.clone();
                let worktree_id = *worktree_id;
                let display_path = short_path(&path);
                Some(
                    div()
                        .id(("file", visible_ix))
                        .w_full()
                        .pl_6()
                        .pr_3()
                        .py_1()
                        .cursor_pointer()
                        .hover(|style| style.bg(cx.theme().colors().element_hover))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.open_file(&path, worktree_id, None, None, _window, cx);
                        }))
                        .child(
                            h_flex()
                                .gap_1p5()
                                .child(
                                    Icon::new(IconName::File)
                                        .size(IconSize::Small)
                                        .color(Color::Muted),
                                )
                                .child(
                                    Label::new(display_path)
                                        .size(LabelSize::Small)
                                        .color(Color::Default),
                                ),
                        )
                        .into_any(),
                )
            }
            KnipEntry::IssueItem {
                file_path,
                name,
                issue_type,
                worktree_id,
                line,
                col,
                ..
            } => {
                let file_path = file_path.clone();
                let name = name.clone();
                let issue_type = *issue_type;
                let worktree_id = *worktree_id;
                let line = *line;
                let col = *col;
                let display_path = short_path(&file_path);
                let location = line
                    .map(|l| {
                        col.map_or_else(|| format!(":{l}"), |c| format!(":{l}:{c}"))
                    })
                    .unwrap_or_default();

                Some(
                    div()
                        .id(("issue", visible_ix))
                        .w_full()
                        .pl_6()
                        .pr_3()
                        .py_1()
                        .cursor_pointer()
                        .hover(|style| style.bg(cx.theme().colors().element_hover))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.open_file(&file_path, worktree_id, line, col, _window, cx);
                        }))
                        .child(
                            h_flex()
                                .gap_1p5()
                                .justify_between()
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .overflow_hidden()
                                        .flex_shrink_1()
                                        .child(
                                            Label::new(name)
                                                .size(LabelSize::Small)
                                                .color(issue_type.color()),
                                        )
                                        .child(
                                            Label::new(format!("{display_path}{location}"))
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted),
                                        ),
                                )
                                .child(
                                    Label::new(issue_type.label())
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                ),
                        )
                        .into_any(),
                )
            }
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let is_running = self.run_state == RunState::Running;

        h_flex()
            .justify_between()
            .px_2()
            .py_1()
            .h(Tab::container_height(cx))
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(
                h_flex()
                    .gap_1p5()
                    .child(Label::new("Knip"))
                    .when(self.run_state == RunState::Done, |el| {
                        el.child(
                            Label::new(format!("{} issues", self.total_issues))
                                .size(LabelSize::XSmall)
                                .color(if self.total_issues > 0 {
                                    Color::Warning
                                } else {
                                    Color::Success
                                }),
                        )
                    }),
            )
            .child(
                Button::new(
                    "run_knip",
                    if is_running { "Running..." } else { "Run" },
                )
                .style(ButtonStyle::Filled)
                .disabled(is_running)
                .on_click(move |_, _window, cx| {
                    entity.update(cx, |this, cx| {
                        this.run_knip(cx);
                    });
                })
                .tooltip(|_window, cx| Tooltip::simple("Run knip to find unused code", cx)),
            )
    }

    fn render_empty_state(&self) -> impl IntoElement {
        v_flex().p_4().gap_2().child(
            div()
                .flex()
                .w_full()
                .items_center()
                .justify_center()
                .child(match self.run_state {
                    RunState::Idle => Label::new("Click \"Run\" to scan for unused code.")
                        .color(Color::Muted)
                        .size(LabelSize::Small),
                    RunState::Running => Label::new("Scanning project...")
                        .color(Color::Muted)
                        .size(LabelSize::Small),
                    RunState::Done => Label::new("No unused code found! Project is clean.")
                        .color(Color::Success)
                        .size(LabelSize::Small),
                    RunState::Error => Label::new(
                        self.error_message
                            .clone()
                            .unwrap_or_else(|| "An unknown error occurred.".to_string()),
                    )
                    .color(Color::Error)
                    .size(LabelSize::Small),
                }),
        )
    }
}

impl Render for KnipPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(point) = self.pending_navigation.take() {
            if let Some(editor) = self
                .workspace
                .upgrade()
                .and_then(|ws| ws.read(cx).active_item(cx))
                .and_then(|item| item.act_as::<Editor>(cx))
            {
                editor.update(cx, |editor, cx| {
                    editor.go_to_singleton_buffer_point(point, window, cx);
                });
            }
        }

        v_flex()
            .id("knip-panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(self.render_header(cx))
            .map(|this| {
                if self.visible_entries.is_empty() && self.all_entries.is_empty() {
                    this.child(self.render_empty_state())
                } else {
                    this.child(
                        list(
                            self.entry_list.clone(),
                            cx.processor(|this, ix, window, cx| {
                                this.render_entry(ix, window, cx)
                                    .unwrap_or_else(|| div().into_any())
                            }),
                        )
                        .size_full(),
                    )
                }
            })
    }
}

impl Focusable for KnipPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for KnipPanel {}

impl Panel for KnipPanel {
    fn persistent_name() -> &'static str {
        "KnipPanel"
    }

    fn panel_key() -> &'static str {
        KNIP_PANEL_KEY
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(
            position,
            DockPosition::Left | DockPosition::Right | DockPosition::Bottom
        )
    }

    fn set_position(
        &mut self,
        position: DockPosition,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.position = position;
        // Trigger SettingsStore global observer so the dock moves this panel
        cx.update_global::<settings::SettingsStore, _>(|_, _| {});
        cx.notify();
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(300.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::FolderSearch)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Knip: Find Unused Code")
    }

    fn icon_label(&self, _window: &Window, _cx: &App) -> Option<String> {
        if self.total_issues > 0 {
            Some(self.total_issues.to_string())
        } else {
            None
        }
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        5
    }
}

fn short_path(path: &str) -> SharedString {
    let path = Path::new(path);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let parent = path
        .parent()
        .map(|p| p.to_string_lossy())
        .unwrap_or_default();
    if parent.is_empty() {
        file_name.into_owned().into()
    } else {
        format!("{parent}/{file_name}").into()
    }
}

// --- Knip JSON parsing ---

#[derive(Debug, Deserialize)]
struct KnipReport {
    #[serde(default)]
    issues: Vec<KnipFileIssues>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct KnipIssueItem {
    name: String,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    col: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct KnipFileIssues {
    file: String,
    #[serde(default)]
    files: Vec<KnipIssueItem>,
    #[serde(default)]
    dependencies: Vec<KnipIssueItem>,
    #[serde(default, rename = "devDependencies")]
    dev_dependencies: Vec<KnipIssueItem>,
    #[serde(default)]
    unlisted: Vec<KnipIssueItem>,
    #[serde(default)]
    unresolved: Vec<KnipIssueItem>,
    #[serde(default)]
    binaries: Vec<KnipIssueItem>,
    #[serde(default)]
    exports: Vec<KnipIssueItem>,
    #[serde(default)]
    types: Vec<KnipIssueItem>,
    #[serde(default)]
    duplicates: Vec<KnipIssueItem>,
    #[serde(default, rename = "enumMembers")]
    enum_members: Vec<KnipIssueItem>,
    #[serde(default, rename = "optionalPeerDependencies")]
    optional_peer_dependencies: Vec<KnipIssueItem>,
    #[serde(default)]
    catalog: Vec<KnipIssueItem>,
    #[serde(default, rename = "namespaceMembers")]
    namespace_members: Vec<KnipIssueItem>,
}

async fn run_knip_process(
    working_directory: &Path,
    node_runtime: &NodeRuntime,
) -> Result<KnipReport> {
    let npm_command = node_runtime
        .npm_command(
            Some(working_directory),
            "exec",
            &["--", "knip", "--reporter", "json", "--no-progress"],
        )
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "Failed to resolve Node.js runtime. Is Node installed?\n{error}"
            )
        })?;

    let mut command = util::command::new_command(npm_command.path);
    command.args(npm_command.args);
    command.envs(npm_command.env);
    command.current_dir(working_directory);
    command.stdout(util::command::Stdio::piped());
    command.stderr(util::command::Stdio::piped());

    let output = command.output().await.map_err(|error| {
        anyhow::anyhow!("Failed to run knip. Is it installed? (npm install -D knip)\n{error}")
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    if stdout.trim().is_empty() {
        if output.status.success() {
            return Ok(KnipReport {
                issues: Vec::new(),
            });
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "knip exited with status {}:\n{}",
            output.status,
            stderr
        ));
    }

    serde_json::from_str(&stdout).map_err(|error| {
        anyhow::anyhow!("Failed to parse knip JSON output: {error}\nRaw output:\n{stdout}")
    })
}
