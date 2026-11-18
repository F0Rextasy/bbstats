//! Interactive hit regions for mouse hover, click, right-click rename, and wheel routing.

use crate::config::DockPosition;
use crate::model::{JobId, Language, ProcessKey, ProjectId};
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;

#[derive(Debug, Clone, PartialEq)]
pub enum HitAction {
    ExpandPanel,
    CollapsePanel,
    ToggleSticky,
    ToggleShowAllProcesses,
    OpenSettings,
    BackToExpanded,
    FocusSearch,
    ClearSearch,
    OpenProjectDetail(ProjectId),
    OpenJobDetail(ProjectId, JobId),
    OpenProjectFolder(ProjectId),
    TogglePinProject(ProjectId),
    CycleSortMode,
    MoveProjectOrder(ProjectId, i32),
    MoveJobOrder(String, i32),
    OpenPort { port: u16, url: Option<String> },
    CopyText(String),
    HideProject(ProjectId),
    RestoreHiddenProjects,
    // Job & custom job actions
    RequestKillJob(JobId),
    ConfirmKillJob(JobId),
    RestartJob(JobId),
    StartCustomJob(JobId),
    RemoveWatchedJob(String),
    BeginRenameJob {
        match_hint: String,
        current_title: String,
    },
    BeginRenameProject {
        project_id: ProjectId,
        current_name: String,
    },
    SaveRename,
    CancelRename,
    BeginAddJob,
    FocusAddJobField(u8),
    SaveAddJob,
    CancelAddJob,
    WatchProcessFromAll {
        match_substr: String,
        title: String,
        working_dir: Option<String>,
    },
    CopyTerminalOutput,
    RefreshTerminalOutput,
    // Project & process kill/restart
    RequestKillProject(ProjectId),
    ConfirmKillProject(ProjectId),
    CancelKillConfirm,
    RequestKillProcess(ProcessKey),
    ConfirmKillProcess(ProcessKey),
    RestartProject(ProjectId),
    DismissAlert(u64),
    RestartAlertProject(ProjectId, u64),
    // Settings actions
    SetLanguage(Language),
    SetDockPosition(DockPosition),
    CycleMonitor,
    ToggleAlwaysOnTop,
    ToggleAcrylic,
    AdjustIdleOpacity(f32),
    AdjustOpacity(f32),
    ToggleFullscreenHide,
    ToggleCrashNotify,
    ToggleHotkeyPreset,
    ToggleAutostart,
    AdjustPeekDelay(i32),
}

#[derive(Debug, Clone)]
pub struct HitRegion {
    pub id: u32,
    pub rect: D2D_RECT_F,
    pub action: HitAction,
    pub right_click_action: Option<HitAction>,
}

#[derive(Debug, Default)]
pub struct HitMap {
    pub regions: Vec<HitRegion>,
    pub scroll_viewport: Option<D2D_RECT_F>,
    pub term_viewport: Option<D2D_RECT_F>,
}

impl HitMap {
    pub fn clear(&mut self) {
        self.regions.clear();
        self.scroll_viewport = None;
        self.term_viewport = None;
    }

    pub fn push(&mut self, id: u32, rect: D2D_RECT_F, action: HitAction) {
        self.regions.push(HitRegion {
            id,
            rect,
            action,
            right_click_action: None,
        });
    }

    pub fn push_with_rclick(
        &mut self,
        id: u32,
        rect: D2D_RECT_F,
        action: HitAction,
        right_click_action: HitAction,
    ) {
        self.regions.push(HitRegion {
            id,
            rect,
            action,
            right_click_action: Some(right_click_action),
        });
    }

    /// Topmost matching region (last pushed wins for overlapping buttons inside rows).
    pub fn hit_test(&self, x: f32, y: f32) -> Option<&HitRegion> {
        self.regions.iter().rev().find(|r| {
            x >= r.rect.left && x <= r.rect.right && y >= r.rect.top && y <= r.rect.bottom
        })
    }
}
