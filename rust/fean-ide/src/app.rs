//! The application: state, the edit–run–look loop, menus and files (Map unit I-11-a).

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText};
use fean_core::trace::{self, ActorView, Analysis, Edge, Finding, LedgerRow, Report, StepInfo, TreeRow};
use serde_json::{json, Value};

use crate::{auto, editor, views};

// ---- what the IDE can do (also printed by `--features` and checked against the manual) ---------

/// (group, name, how to reach it)
pub const FEATURES: &[(&str, &str, &str)] = &[
    ("편집기", "새 파일", "파일 › 새 파일 (⌘N). 작은 본보기 모델로 시작한다."),
    ("편집기", "파일 열기", "파일 › 열기 (⌘O), 또는 명령줄에서 fean-ide FILE."),
    ("편집기", "저장", "파일 › 저장 (⌘S)."),
    ("편집기", "다른 이름으로 저장", "파일 › 다른 이름으로 저장 (⇧⌘S)."),
    ("편집기", "예제 열기", "파일 › 예제 열기. 명세의 모델 열 개가 들어 있다."),
    ("편집기", "구문 강조", "예약어, 수와 시각, 주석, 기호를 색으로 구별한다."),
    ("편집기", "줄 번호", "편집기 왼쪽의 여백. 오류가 있는 줄에는 표시가 붙는다."),
    ("편집기", "실시간 검사", "입력을 멈추면 곧 구문과 규칙을 검사해 오류의 규칙 이름과 줄을 보여 준다."),
    ("편집기", "문제 목록", "편집기 아래. 항목을 누르면 그 줄로 간다."),
    ("편집기", "찾기", "⌘F. Enter로 다음, ⇧Enter로 이전."),
    ("편집기", "실행 취소와 다시 실행", "⌘Z, ⇧⌘Z."),
    ("실행", "실행", "도구 막대의 실행 단추, ⌘R 또는 F5. 컴파일하고 모든 배정을 실행하고 역산 질의를 풀어 결과를 바꾼다."),
    ("실행", "탐색 한도 설정", "실행 › 탐색 한도. 관찰 지점 수가 한도를 넘으면 미판정(UNKNOWN)이다."),
    ("실행", "낡은 결과 표시", "글이 바뀌었거나 컴파일에 실패하면 이전 결과에 낡음 표시가 붙는다."),
    ("실행", "선택 유지", "다시 실행해도 고른 배정, 주체, 이력을 같은 것으로 다시 찾는다."),
    ("화면", "화면 배치 전환", "편집 · 나란히 · 결과 (⌘1, ⌘2, ⌘3)."),
    ("화면", "밝은 화면과 어두운 화면", "보기 › 어두운 화면."),
    ("화면", "글자 크기 조절", "보기 › 글자 크게 (⌘+), 글자 작게 (⌘−)."),
    ("결과", "사례 선택", "결과 머리의 사례 고르기. 공개 매개변수의 배정마다, 역산의 해와 해가 아닌 값마다 결과가 따로 있다."),
    ("결과", "개요와 질의", "모델의 규모와 질의 결과."),
    ("결과", "증인과 반례 보기", "질의마다 식이 성립하는 이력과 깨지는 이력으로 간다."),
    ("결과", "전략 보기", "can 과 max policy 질의가 찾은 전략을 정보 집합별로 보여 준다."),
    ("결과", "모순", "컴파일 오류, 실행 오류, 실패한 필수 사건, 깨진 보장, 해가 없는 역산 질의를 모은다."),
    ("결과", "역산", "미지수와 solve 질의의 결과. 질의마다 답, 모델을 실행한 횟수, 판정이 같은 영역의 표."),
    ("결과", "해와 반례 보기", "역산 보기의 단추. 해나 해가 아닌 값에서 실행한 모델을 사례로 골라 다른 보기에서 본다."),
    ("결과", "이 값으로 보기", "역산 보기의 영역마다. 그 영역의 표본 값에서 모델을 실행해 사례로 연다."),
    ("결과", "거래 그래프", "누가 누구에게 얼마를 보냈는지. 재생 위치까지의 합계."),
    ("결과", "시점별 구조와 재생", "주체별 줄 위에 걸음을 시간순으로 놓고 한 걸음씩 재생한다."),
    ("결과", "재생 조작", "처음, 이전, 재생·멈춤, 다음, 끝, 속도, 위치 막대."),
    ("결과", "주체", "한 주체의 보유량, 받은 통지, 보이는 메뉴."),
    ("결과", "거래 내역", "주체가 낀 이전과 자산별 잔액."),
    ("결과", "지식 보기", "주체가 가능하다고 보는 세계와, 아는 값·모르는 값."),
    ("결과", "이력", "모든 완결 이력의 표. 실패한 이력만 볼 수 있다."),
    ("결과", "이력 나무", "이력이 갈리는 지점(세계, 가지, 선택)의 나무."),
    ("결과", "컴파일 결과", "이미지의 규모와 목록."),
    ("결과", "이미지 저장", "컴파일 결과 보기의 단추. .feanc 파일로 쓴다."),
    ("결과", "코드로 이동", "결과의 사건, 셀, 질의, 모순, 걸음을 누르면 편집기가 그 줄로 간다."),
    ("명령", "자동 실행 모드", "fean-ide --shots DIR. 시나리오를 수행하며 화면과 보고서를 남긴다."),
    ("명령", "기능 목록 출력", "fean-ide --features."),
    ("명령", "단축키", "도움말 › 단축키."),
];

pub fn features_json() -> String {
    let rows: Vec<Value> = FEATURES.iter().map(|(group, name, how)| json!({"group": group, "name": name, "how": how})).collect();
    serde_json::to_string_pretty(&rows).unwrap_or_default()
}

/// (model, what it shows, text): the models of the specification, built in.
pub const EXAMPLES: &[(&str, &str, &str)] = &[
    ("Order", "한 번의 선택과 운", include_str!("../../../examples/order.fean")),
    ("MutualDebt", "서로 진 빚: 총액 결제와 순액 결제", include_str!("../../../examples/mutualdebt.fean")),
    ("Support", "지원 조건의 순환과 결제 불능", include_str!("../../../examples/support.fean")),
    ("LoanLifecycle", "대출의 일생 (표준 계약 Loan)", include_str!("../../../examples/loanlifecycle.fean")),
    ("CallExample", "콜 옵션 (표준 계약 Call)", include_str!("../../../examples/callexample.fean")),
    ("Repay80", "통지 시점, 선택, 상환", include_str!("../../../examples/repay80.fean")),
    ("Straddle", "매개변수의 영역", include_str!("../../../examples/straddle.fean")),
    ("Workout", "선호와 확률 가중", include_str!("../../../examples/workout.fean")),
    ("Capital", "역산: 필요한 자본", include_str!("../../../examples/capital.fean")),
    ("Escrow", "역산: 지점의 상태", include_str!("../../../examples/escrow.fean")),
];

pub const TEMPLATE: &str = "model Untitled {
  actor A, B
  asset KRW in 0..100 init A = 100
  time 0..1

  event pay by A { move A -> B : 30[KRW] }

  program {
    at @1 { run pay }
  }

  query paid = forall run: at(end, bal(B, KRW) == 30[KRW])
}
";

pub const SHORTCUTS: &[(&str, &str)] = &[
    ("⌘R / F5", "실행"),
    ("⌘S", "저장"),
    ("⇧⌘S", "다른 이름으로 저장"),
    ("⌘O", "열기"),
    ("⌘N", "새 파일"),
    ("⌘F", "찾기 (Enter 다음, ⇧Enter 이전, Esc 닫기)"),
    ("⌘1 · ⌘2 · ⌘3", "화면 배치: 편집 · 나란히 · 결과"),
    ("⌘+ · ⌘−", "글자 크게 · 작게"),
    ("← · →", "재생 위치를 한 걸음 뒤로 · 앞으로 (편집기 밖에서)"),
    ("Space", "재생 · 멈춤 (편집기 밖에서)"),
    ("⌘Z · ⇧⌘Z", "실행 취소 · 다시 실행"),
];

// ---- look --------------------------------------------------------------------------------------

pub struct Palette {
    pub accent: Color32,
    pub error: Color32,
    pub warn: Color32,
    pub ok: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub panel: Color32,
    pub text: Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            accent: Color32::from_rgb(96, 165, 250),
            error: Color32::from_rgb(248, 113, 113),
            warn: Color32::from_rgb(251, 191, 36),
            ok: Color32::from_rgb(52, 211, 153),
            muted: Color32::from_rgb(148, 163, 184),
            faint: Color32::from_rgb(51, 65, 85),
            panel: Color32::from_rgb(30, 41, 59),
            text: Color32::from_rgb(226, 232, 240),
        }
    } else {
        Palette {
            accent: Color32::from_rgb(37, 99, 235),
            error: Color32::from_rgb(220, 38, 38),
            warn: Color32::from_rgb(217, 119, 6),
            ok: Color32::from_rgb(5, 150, 105),
            muted: Color32::from_rgb(100, 116, 139),
            faint: Color32::from_rgb(226, 232, 240),
            panel: Color32::from_rgb(248, 250, 252),
            text: Color32::from_rgb(15, 23, 42),
        }
    }
}

const ACTOR_COLORS: [Color32; 8] = [
    Color32::from_rgb(37, 99, 235),
    Color32::from_rgb(234, 88, 12),
    Color32::from_rgb(5, 150, 105),
    Color32::from_rgb(124, 58, 237),
    Color32::from_rgb(219, 39, 119),
    Color32::from_rgb(13, 148, 136),
    Color32::from_rgb(202, 138, 4),
    Color32::from_rgb(71, 85, 105),
];

pub fn actor_color(actor: usize) -> Color32 {
    ACTOR_COLORS[actor % ACTOR_COLORS.len()]
}

/// Fonts and spacing. Korean text needs a system font: without one the labels are still drawn,
/// but Hangul shows as boxes.
pub fn setup(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for path in [
        "/System/Library/Fonts/AppleSDGothicNeo.ttc",
        "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
        "/Library/Fonts/Arial Unicode.ttf",
        "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
        "C:\\Windows\\Fonts\\malgun.ttf",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert("korean".to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
            fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "korean".to_owned());
            fonts.families.entry(egui::FontFamily::Monospace).or_default().push("korean".to_owned());
            break;
        }
    }
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Light);
    apply_style(ctx, 14.0);
}

pub fn apply_style(ctx: &egui::Context, size: f32) {
    ctx.all_styles_mut(|style| {
        use egui::{FontFamily, FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Heading, FontId::new(size + 5.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(size, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(size - 0.5, FontFamily::Monospace)),
            (TextStyle::Button, FontId::new(size, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(size - 2.5, FontFamily::Proportional)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.interaction.selectable_labels = false;
    });
}

// ---- state -------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layout {
    Edit,
    Split,
    Results,
}

impl Layout {
    pub fn name(self) -> &'static str {
        match self {
            Layout::Edit => "편집",
            Layout::Split => "나란히",
            Layout::Results => "결과",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Overview,
    Findings,
    Solve,
    Graph,
    Timeline,
    Actor,
    Histories,
    Compile,
}

impl View {
    pub const ALL: [View; 8] = [
        View::Overview,
        View::Findings,
        View::Solve,
        View::Graph,
        View::Timeline,
        View::Actor,
        View::Histories,
        View::Compile,
    ];

    pub fn name(self) -> &'static str {
        match self {
            View::Overview => "개요와 질의",
            View::Findings => "모순",
            View::Solve => "역산",
            View::Graph => "거래 그래프",
            View::Timeline => "시점별 구조와 재생",
            View::Actor => "주체",
            View::Histories => "이력",
            View::Compile => "컴파일 결과",
        }
    }
}

/// What a click in the results asks for; applied after the frame is drawn.
pub enum Act {
    Run,
    View(View),
    Assignment(usize),
    SelectRun(usize),
    Step(usize),
    Actor(usize),
    Jump(u32),
    Play(bool),
    Speed(f32),
    /// Go to a history (and a step of it) in a view.
    Show { assignment: usize, run: usize, step: Option<usize>, view: View },
    SaveImage,
    FailedOnly(bool),
    OnlyActor(bool),
    /// Look at a candidate of a solve: (solve, row of the assignment, the candidate).
    Case { solve: usize, row: usize, point: Vec<i64> },
}

#[derive(Clone, Debug)]
pub struct Diag {
    pub kind: String,
    pub code: String,
    pub line: Option<u32>,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub assignment: usize,
    pub run: usize,
    pub step: usize,
    pub actor: usize,
}

/// What the views show for the current selection, computed when the selection changes.
#[derive(Default)]
pub struct Cache {
    steps_key: Option<(u64, usize, usize)>,
    pub steps: Vec<StepInfo>,
    tree_key: Option<(u64, usize)>,
    pub tree: Vec<TreeRow>,
    graph_key: Option<(u64, usize, usize, usize)>,
    pub graph: Vec<Edge>,
    ledger_key: Option<(u64, usize, usize, usize)>,
    pub ledger: Vec<LedgerRow>,
    view_key: Option<(u64, usize, usize, usize, usize)>,
    pub view: Option<ActorView>,
}

#[derive(Default)]
pub struct Find {
    pub open: bool,
    pub query: String,
    pub focus: bool,
    /// Ask the editor to select the next (true) or previous (false) match.
    pub go: Option<bool>,
    pub count: usize,
}

struct RunDone {
    text_gen: u64,
    report: Report,
    ms: f64,
}

pub struct IdeApp {
    // the text
    pub text: String,
    pub path: Option<PathBuf>,
    saved_text: String,
    /// Counts the changes of the text.
    pub text_gen: u64,
    edit_time: f64,
    pub diagnostics: Vec<Diag>,
    checked_gen: u64,
    check_running: bool,
    check_tx: Sender<(u64, Option<Diag>)>,
    check_rx: Receiver<(u64, Option<Diag>)>,
    /// A line the editor should move to (1-based).
    pub goto: Option<u32>,
    /// The line last jumped to and when, to mark it for a moment.
    pub flash: Option<(u32, f64)>,
    pub cursor: (usize, usize),
    pub find: Find,

    // running
    pub running: bool,
    run_tx: Sender<RunDone>,
    run_rx: Receiver<RunDone>,
    pub budget: usize,
    pub analysis: Option<Analysis>,
    pub findings: Vec<Finding>,
    /// The compile error of the last run, if it failed (the older results stay, marked stale).
    pub run_error: Option<Finding>,
    /// `text_gen` of the text the results were made from.
    pub result_text_gen: u64,
    /// Counts the results (a new number for every successful run).
    pub result_gen: u64,
    pub run_ms: f64,
    pub runs_done: usize,

    // looking
    pub layout: Layout,
    pub view: View,
    pub sel: Selection,
    pub cache: Cache,
    pub playing: bool,
    pub speed: f32,
    play_at: f64,
    pub failed_only: bool,
    pub only_actor: bool,

    // the rest
    pub status: String,
    pub dark: bool,
    pub font_size: f32,
    pub now: f64,
    show_features: bool,
    show_shortcuts: bool,
    pub allow_close: bool,
    pub auto: Option<auto::Auto>,
}

impl IdeApp {
    pub fn new(file: Option<PathBuf>, shots: Option<PathBuf>) -> Self {
        let (check_tx, check_rx) = channel();
        let (run_tx, run_rx) = channel();
        let mut app = IdeApp {
            text: String::new(),
            path: None,
            saved_text: String::new(),
            text_gen: 1,
            edit_time: 0.0,
            diagnostics: Vec::new(),
            checked_gen: 0,
            check_running: false,
            check_tx,
            check_rx,
            goto: None,
            flash: None,
            cursor: (1, 1),
            find: Find::default(),
            running: false,
            run_tx,
            run_rx,
            budget: fean_core::vm::DEFAULT_BUDGET,
            analysis: None,
            findings: Vec::new(),
            run_error: None,
            result_text_gen: 0,
            result_gen: 0,
            run_ms: 0.0,
            runs_done: 0,
            layout: Layout::Split,
            view: View::Overview,
            sel: Selection::default(),
            cache: Cache::default(),
            playing: false,
            speed: 1.0,
            play_at: 0.0,
            failed_only: false,
            only_actor: false,
            status: "준비됨. 실행(⌘R)을 누르면 결과가 오른쪽에 나옵니다.".to_string(),
            dark: false,
            font_size: 14.0,
            now: 0.0,
            show_features: false,
            show_shortcuts: false,
            allow_close: false,
            auto: shots.map(auto::Auto::new),
        };
        match file {
            Some(path) => app.open_path(path),
            None => {
                let (_, _, text) = EXAMPLES[5];
                app.set_document(text.to_string(), None);
                app.status = "예제 Repay80 을 열었습니다. 실행(⌘R)을 누르면 결과가 오른쪽에 나옵니다.".to_string();
            }
        }
        app
    }

    // ---- the text ----------------------------------------------------------------------------
    pub fn dirty(&self) -> bool {
        self.text != self.saved_text
    }

    /// The text changed: check it again soon.
    pub fn touched(&mut self) {
        self.text_gen += 1;
        self.edit_time = self.now;
    }

    /// Replace the whole document (a file was opened, or a new one started).
    pub fn set_document(&mut self, text: String, path: Option<PathBuf>) {
        self.saved_text = text.clone();
        self.text = text;
        self.path = path;
        self.diagnostics.clear();
        self.analysis = None;
        self.findings.clear();
        self.run_error = None;
        self.cache = Cache::default();
        self.sel = Selection::default();
        self.playing = false;
        self.touched();
        self.edit_time = -1.0; // check at once
    }

    /// Change the text as if it had been typed (the automatic mode edits this way).
    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.touched();
    }

    pub fn file_name(&self) -> String {
        match &self.path {
            Some(p) => p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.display().to_string()),
            None => "이름 없음.fean".to_string(),
        }
    }

    pub fn open_path(&mut self, path: PathBuf) {
        match std::fs::read(&path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => {
                    let text = if text.contains('\r') { text.replace("\r\n", "\n").replace('\r', "\n") } else { text };
                    self.status = format!("{} 을 열었습니다.", path.display());
                    self.set_document(text, Some(path));
                }
                Err(_) => self.status = format!("{}: UTF-8 글이 아니어서 열지 못했습니다.", path.display()),
            },
            Err(e) => {
                self.status = format!("{}: 열지 못했습니다 ({e}).", path.display());
                if self.text.is_empty() {
                    self.set_document(TEMPLATE.to_string(), None);
                }
            }
        }
    }

    /// Ask before throwing away unsaved changes. True when it is fine to go on.
    fn may_discard(&mut self) -> bool {
        if !self.dirty() || self.auto.is_some() {
            return true;
        }
        let answer = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("저장하지 않은 변경")
            .set_description(format!("{} 에 저장하지 않은 변경이 있습니다. 저장할까요?", self.file_name()))
            .set_buttons(rfd::MessageButtons::YesNoCancelCustom("저장".into(), "저장 안 함".into(), "취소".into()))
            .show();
        match answer {
            rfd::MessageDialogResult::Yes => self.save(),
            rfd::MessageDialogResult::Custom(text) if text == "저장" => self.save(),
            rfd::MessageDialogResult::No => true,
            rfd::MessageDialogResult::Custom(text) if text == "저장 안 함" => true,
            _ => false,
        }
    }

    pub fn new_file(&mut self) {
        if self.may_discard() {
            self.set_document(TEMPLATE.to_string(), None);
            self.status = "새 파일을 시작했습니다.".to_string();
        }
    }

    pub fn open_dialog(&mut self) {
        if !self.may_discard() {
            return;
        }
        let mut dialog = rfd::FileDialog::new().set_title("모델 글 열기").add_filter("FEAN 모델", &["fean"]).add_filter("모든 파일", &["*"]);
        if let Some(dir) = self.path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.open_path(path);
        }
    }

    pub fn load_example(&mut self, index: usize) {
        if self.may_discard() {
            let (name, _, text) = EXAMPLES[index];
            self.set_document(text.to_string(), None);
            self.status = format!("예제 {name} 을 열었습니다.");
        }
    }

    /// Save to the file's path (asking for one first if there is none). True when saved.
    pub fn save(&mut self) -> bool {
        match self.path.clone() {
            Some(path) => self.save_to(path),
            None => self.save_as(),
        }
    }

    pub fn save_as(&mut self) -> bool {
        let mut dialog = rfd::FileDialog::new().set_title("다른 이름으로 저장").add_filter("FEAN 모델", &["fean"]).set_file_name(self.file_name());
        if let Some(dir) = self.path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        match dialog.save_file() {
            Some(path) => self.save_to(path),
            None => false,
        }
    }

    pub fn save_to(&mut self, path: PathBuf) -> bool {
        match std::fs::write(&path, &self.text) {
            Ok(()) => {
                self.saved_text = self.text.clone();
                self.status = format!("{} 에 저장했습니다.", path.display());
                self.path = Some(path);
                true
            }
            Err(e) => {
                self.status = format!("{}: 저장하지 못했습니다 ({e}).", path.display());
                false
            }
        }
    }

    fn save_image(&mut self) {
        let Some(analysis) = &self.analysis else { return };
        let name = format!("{}.feanc", analysis.image.model.to_lowercase());
        let dialog = rfd::FileDialog::new().set_title("이미지 저장").add_filter("FEAN 이미지", &["feanc"]).set_file_name(name);
        if let Some(path) = dialog.save_file() {
            self.status = match std::fs::write(&path, &analysis.image_text) {
                Ok(()) => format!("이미지를 {} 에 저장했습니다.", path.display()),
                Err(e) => format!("{}: 저장하지 못했습니다 ({e}).", path.display()),
            };
        }
    }

    // ---- checking while typing ---------------------------------------------------------------
    fn schedule_check(&mut self, ctx: &egui::Context) {
        if self.checked_gen == self.text_gen || self.check_running {
            return;
        }
        if self.now - self.edit_time < 0.3 {
            ctx.request_repaint_after(Duration::from_millis(120));
            return;
        }
        self.check_running = true;
        let (text, gen, tx, ctx) = (self.text.clone(), self.text_gen, self.check_tx.clone(), ctx.clone());
        // the front end recurses with the nesting of the text: give it a deep stack
        let spawned = std::thread::Builder::new().name("fean-check".into()).stack_size(256 << 20).spawn(move || {
            let diag = fean_core::frontend(&text, None).err().map(|e| Diag {
                kind: e.kind.name().to_string(),
                code: e.code,
                line: e.line,
                message: e.message,
            });
            let _ = tx.send((gen, diag));
            ctx.request_repaint();
        });
        if spawned.is_err() {
            self.check_running = false;
            self.status = "검사 스레드를 시작하지 못했습니다.".to_string();
        }
    }

    pub fn checked(&self) -> bool {
        self.checked_gen == self.text_gen
    }

    // ---- running -----------------------------------------------------------------------------
    pub fn run(&mut self, ctx: &egui::Context) {
        if self.running {
            return;
        }
        self.running = true;
        self.status = "컴파일하고 실행하는 중…".to_string();
        let (text, text_gen, budget, tx, ctx) = (self.text.clone(), self.text_gen, self.budget, self.run_tx.clone(), ctx.clone());
        let spawned = std::thread::Builder::new().name("fean-run".into()).stack_size(256 << 20).spawn(move || {
            let start = Instant::now();
            let report = trace::analyze(&text, None, budget);
            let _ = tx.send(RunDone { text_gen, report, ms: start.elapsed().as_secs_f64() * 1000.0 });
            ctx.request_repaint();
        });
        if spawned.is_err() {
            self.running = false;
            self.status = "실행 스레드를 시작하지 못했습니다.".to_string();
        }
    }

    fn poll(&mut self) {
        while let Ok((gen, diag)) = self.check_rx.try_recv() {
            self.check_running = false;
            if gen == self.text_gen {
                self.checked_gen = gen;
                self.diagnostics = diag.into_iter().collect();
            }
        }
        while let Ok(done) = self.run_rx.try_recv() {
            self.running = false;
            self.adopt(done);
        }
    }

    /// Who is selected, by name: survives a rerun.
    pub fn identity(&self) -> Option<(String, String, String, usize)> {
        let a = self.analysis.as_ref()?;
        let slot = a.slots.get(self.sel.assignment)?;
        let actor = a.image.actors.get(self.sel.actor).cloned().unwrap_or_default();
        let key = slot.runs.get(self.sel.run).map(|r| r.key.clone()).unwrap_or_default();
        Some((slot.label.clone(), actor, key, self.sel.step))
    }

    fn adopt(&mut self, done: RunDone) {
        self.run_ms = done.ms;
        self.runs_done += 1;
        match done.report.analysis {
            Some(analysis) => {
                let before = self.identity();
                let queries = analysis.answers().len();
                let contradictions = done.report.findings.iter().filter(|f| f.kind != "unknown").count();
                self.findings = done.report.findings;
                self.analysis = Some(analysis);
                self.run_error = None;
                self.result_text_gen = done.text_gen;
                self.result_gen += 1;
                self.cache = Cache::default();
                self.restore(before);
                let solves = self.analysis.as_ref().map_or(0, |a| a.solves().len());
                let solved = if solves > 0 { format!(", 역산 {solves}개") } else { String::new() };
                self.status = format!("실행 완료: 질의 {queries}개{solved}, 모순 {contradictions}건 ({:.0} ms).", done.ms);
            }
            None => {
                let error = done.report.findings.into_iter().next();
                if let Some(f) = &error {
                    self.status = format!("{} — {}. 이전 결과는 낡은 채로 둡니다.", f.title, f.detail);
                    if let Some(line) = f.loc.map(|l| l.line) {
                        self.goto = Some(line);
                    }
                }
                self.run_error = error;
            }
        }
    }

    /// Find the same assignment, actor and history again in the new results.
    fn restore(&mut self, before: Option<(String, String, String, usize)>) {
        let Some(a) = &self.analysis else { return };
        let Some((label, actor, key, step)) = before else {
            self.sel = Selection::default();
            return;
        };
        let assignment = a.slots.iter().position(|s| s.label == label).unwrap_or(0);
        let runs = a.slots.get(assignment).map(|s| s.runs.as_slice()).unwrap_or(&[]);
        let run = runs.iter().position(|r| r.key == key).unwrap_or_else(|| {
            // no history with the same choices: the one that shares the longest beginning
            let shared = |other: &str| key.split('/').zip(other.split('/')).take_while(|(x, y)| x == y).count();
            (0..runs.len()).max_by_key(|&i| (shared(&runs[i].key), std::cmp::Reverse(i))).unwrap_or(0)
        });
        self.sel = Selection { assignment, run, step, actor: a.image.actors.iter().position(|n| *n == actor).unwrap_or(0) };
    }

    /// The results were made from an older text.
    pub fn stale(&self) -> bool {
        self.analysis.is_some() && self.result_text_gen != self.text_gen
    }

    /// Bring what the views show in line with the selection.
    pub fn refresh(&mut self) {
        let Some(a) = self.analysis.as_mut() else { return };
        if self.sel.assignment >= a.slots.len() {
            self.sel.assignment = 0;
        }
        let _ = a.select(self.sel.assignment);
        if self.sel.run >= a.runs().len() {
            self.sel.run = 0;
        }
        if self.sel.actor >= a.image.actors.len() {
            self.sel.actor = 0;
        }
        let gen = self.result_gen;
        let (assignment, run, actor) = (self.sel.assignment, self.sel.run, self.sel.actor);
        if self.cache.steps_key != Some((gen, assignment, run)) {
            self.cache.steps = a.steps(run);
            self.cache.steps_key = Some((gen, assignment, run));
        }
        if self.sel.step >= self.cache.steps.len() {
            self.sel.step = self.cache.steps.len().saturating_sub(1);
        }
        let step = self.sel.step;
        if self.cache.tree_key != Some((gen, assignment)) {
            self.cache.tree = a.tree();
            self.cache.tree_key = Some((gen, assignment));
        }
        if self.cache.graph_key != Some((gen, assignment, run, step)) {
            self.cache.graph = a.graph(run, step);
            self.cache.graph_key = Some((gen, assignment, run, step));
        }
        if self.cache.ledger_key != Some((gen, assignment, run, actor)) {
            self.cache.ledger = a.ledger(run, actor);
            self.cache.ledger_key = Some((gen, assignment, run, actor));
        }
        if self.cache.view_key != Some((gen, assignment, run, step, actor)) {
            self.cache.view = a.actor_view(run, step, actor);
            self.cache.view_key = Some((gen, assignment, run, step, actor));
        }
    }

    /// Run the model at a candidate of a solve and look at it: it becomes the selected case.
    pub fn open_case(&mut self, solve: usize, row: usize, point: &[i64]) {
        let Some(a) = self.analysis.as_mut() else { return };
        match a.open_case(solve, row, point) {
            Ok(case) => {
                let label = a.slots[case].label.clone();
                self.sel = Selection { assignment: case, run: 0, step: usize::MAX, actor: self.sel.actor };
                self.cache = Cache::default(); // the case took the place of one picked before
                self.playing = false;
                self.status = format!("사례를 열었습니다: {label}. 다른 보기에서 이 사례의 이력을 볼 수 있습니다.");
            }
            Err(e) => self.status = format!("이 값은 사례로 열 수 없습니다: {}", e.message),
        }
    }

    /// Move the editor to a line of the text (and make sure the editor is on screen).
    pub fn jump(&mut self, line: u32) {
        self.goto = Some(line);
        if self.layout == Layout::Results {
            self.layout = Layout::Split;
        }
    }

    pub fn apply(&mut self, act: Act, ctx: &egui::Context) {
        match act {
            Act::Run => self.run(ctx),
            Act::View(v) => self.view = v,
            Act::Assignment(a) => {
                self.sel = Selection { assignment: a, run: 0, step: 0, actor: self.sel.actor };
                self.playing = false;
            }
            Act::SelectRun(r) => {
                self.sel.run = r;
                self.sel.step = usize::MAX; // the end of the history
                self.playing = false;
            }
            Act::Step(s) => self.sel.step = s,
            Act::Actor(a) => self.sel.actor = a,
            Act::Jump(line) => self.jump(line),
            Act::Play(on) => {
                self.playing = on;
                self.play_at = self.now;
                if on && self.sel.step + 1 >= self.cache.steps.len() {
                    self.sel.step = 0;
                }
            }
            Act::Speed(s) => self.speed = s,
            Act::Show { assignment, run, step, view } => {
                self.sel.assignment = assignment;
                self.sel.run = run;
                self.sel.step = step.unwrap_or(usize::MAX);
                self.view = view;
                self.playing = false;
            }
            Act::SaveImage => self.save_image(),
            Act::FailedOnly(on) => self.failed_only = on,
            Act::OnlyActor(on) => self.only_actor = on,
            Act::Case { solve, row, point } => self.open_case(solve, row, &point),
        }
    }

    // ---- frame -------------------------------------------------------------------------------
    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let hit = |ctx: &egui::Context, mods: Modifiers, key: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(mods, key)));
        let shift_cmd = Modifiers::COMMAND | Modifiers::SHIFT;
        if hit(ctx, shift_cmd, Key::S) {
            self.save_as();
        } else if hit(ctx, Modifiers::COMMAND, Key::S) {
            self.save();
        }
        if hit(ctx, Modifiers::COMMAND, Key::O) {
            self.open_dialog();
        }
        if hit(ctx, Modifiers::COMMAND, Key::N) {
            self.new_file();
        }
        if hit(ctx, Modifiers::COMMAND, Key::R) || hit(ctx, Modifiers::NONE, Key::F5) {
            self.run(ctx);
        }
        if hit(ctx, Modifiers::COMMAND, Key::F) {
            self.find.open = true;
            self.find.focus = true;
            if self.layout == Layout::Results {
                self.layout = Layout::Split;
            }
        }
        for (key, layout) in [(Key::Num1, Layout::Edit), (Key::Num2, Layout::Split), (Key::Num3, Layout::Results)] {
            if hit(ctx, Modifiers::COMMAND, key) {
                self.layout = layout;
            }
        }
        if hit(ctx, Modifiers::COMMAND, Key::Equals) || hit(ctx, Modifiers::COMMAND, Key::Plus) {
            self.set_font_size(ctx, self.font_size + 1.0);
        }
        if hit(ctx, Modifiers::COMMAND, Key::Minus) {
            self.set_font_size(ctx, self.font_size - 1.0);
        }
        // replay keys, when no text field has the keyboard
        if self.analysis.is_some() && ctx.memory(|m| m.focused().is_none()) {
            if hit(ctx, Modifiers::NONE, Key::ArrowRight) {
                self.playing = false;
                self.sel.step = (self.sel.step + 1).min(self.cache.steps.len().saturating_sub(1));
            }
            if hit(ctx, Modifiers::NONE, Key::ArrowLeft) {
                self.playing = false;
                self.sel.step = self.sel.step.saturating_sub(1);
            }
            if hit(ctx, Modifiers::NONE, Key::Space) {
                self.apply(Act::Play(!self.playing), ctx);
            }
        }
    }

    fn set_font_size(&mut self, ctx: &egui::Context, size: f32) {
        self.font_size = size.clamp(10.0, 24.0);
        apply_style(ctx, self.font_size);
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        let pal = palette(self.dark);
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            let close = |ui: &egui::Ui| ui.close_kind(egui::UiKind::Menu);
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("파일", |ui| {
                    if ui.button("새 파일    ⌘N").clicked() {
                        close(ui);
                        self.new_file();
                    }
                    if ui.button("열기…    ⌘O").clicked() {
                        close(ui);
                        self.open_dialog();
                    }
                    ui.menu_button("예제 열기", |ui| {
                        for (i, (name, what, _)) in EXAMPLES.iter().enumerate() {
                            if ui.button(format!("{name} — {what}")).clicked() {
                                close(ui);
                                self.load_example(i);
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("저장    ⌘S").clicked() {
                        close(ui);
                        self.save();
                    }
                    if ui.button("다른 이름으로 저장…    ⇧⌘S").clicked() {
                        close(ui);
                        self.save_as();
                    }
                });
                ui.menu_button("실행", |ui| {
                    if ui.add_enabled(!self.running, egui::Button::new("실행    ⌘R")).clicked() {
                        close(ui);
                        self.run(ctx);
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("탐색 한도");
                        ui.add(egui::DragValue::new(&mut self.budget).range(1..=5_000_000).speed(1000.0))
                            .on_hover_text("배정마다 만들 관찰 지점의 최대 수. 넘으면 그 배정의 질의는 UNKNOWN 입니다.");
                    });
                });
                ui.menu_button("보기", |ui| {
                    for layout in [Layout::Edit, Layout::Split, Layout::Results] {
                        if ui.radio(self.layout == layout, format!("배치: {}", layout.name())).clicked() {
                            self.layout = layout;
                            close(ui);
                        }
                    }
                    ui.separator();
                    for view in View::ALL {
                        if ui.radio(self.view == view, view.name()).clicked() {
                            self.view = view;
                            if self.layout == Layout::Edit {
                                self.layout = Layout::Split;
                            }
                            close(ui);
                        }
                    }
                    ui.separator();
                    if ui.checkbox(&mut self.dark, "어두운 화면").changed() {
                        ctx.set_theme(if self.dark { egui::Theme::Dark } else { egui::Theme::Light });
                    }
                    if ui.button("글자 크게    ⌘+").clicked() {
                        self.set_font_size(ctx, self.font_size + 1.0);
                    }
                    if ui.button("글자 작게    ⌘−").clicked() {
                        self.set_font_size(ctx, self.font_size - 1.0);
                    }
                });
                ui.menu_button("도움말", |ui| {
                    if ui.button("기능 목록").clicked() {
                        self.show_features = true;
                        close(ui);
                    }
                    if ui.button("단축키").clicked() {
                        self.show_shortcuts = true;
                        close(ui);
                    }
                });
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let run = egui::Button::new(RichText::new("▶  실행").color(Color32::WHITE).strong()).fill(pal.accent);
                if ui.add_enabled(!self.running, run).on_hover_text("컴파일하고 실행합니다 (⌘R)").clicked() {
                    self.run(ctx);
                }
                ui.separator();
                for layout in [Layout::Edit, Layout::Split, Layout::Results] {
                    if ui.selectable_label(self.layout == layout, layout.name()).clicked() {
                        self.layout = layout;
                    }
                }
                ui.separator();
                let name = if self.dirty() { format!("{}  ●", self.file_name()) } else { self.file_name() };
                ui.label(RichText::new(name).strong()).on_hover_text(match &self.path {
                    Some(p) => p.display().to_string(),
                    None => "아직 저장하지 않은 글".to_string(),
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (text, color) = if self.running {
                        ("실행 중…", pal.accent)
                    } else if self.analysis.is_none() {
                        ("결과 없음", pal.muted)
                    } else if self.stale() {
                        ("결과 낡음", pal.warn)
                    } else {
                        ("결과 최신", pal.ok)
                    };
                    ui.label(RichText::new(text).color(color).strong());
                    if self.running {
                        ui.spinner();
                    }
                });
            });
            ui.add_space(2.0);
        });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        let pal = palette(self.dark);
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&self.status).small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("줄 {}, 칸 {}", self.cursor.0, self.cursor.1)).small().color(pal.muted));
                    ui.separator();
                    let (text, color) = match (self.checked(), self.diagnostics.first()) {
                        (false, _) => ("검사 중…".to_string(), pal.muted),
                        (true, None) => ("문제 없음".to_string(), pal.ok),
                        (true, Some(d)) => (format!("문제 {}건: [{}]", self.diagnostics.len(), d.code), pal.error),
                    };
                    ui.label(RichText::new(text).small().color(color));
                });
            });
        });
    }

    fn windows(&mut self, ctx: &egui::Context) {
        let pal = palette(self.dark);
        let mut features = self.show_features;
        egui::Window::new("기능 목록").open(&mut features).default_size([620.0, 520.0]).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                let mut group = "";
                for (g, name, how) in FEATURES {
                    if *g != group {
                        group = g;
                        ui.add_space(6.0);
                        ui.label(RichText::new(group).strong().color(pal.accent));
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(*name).strong());
                        ui.label(RichText::new(*how).color(pal.muted));
                    });
                }
            });
        });
        self.show_features = features;
        let mut shortcuts = self.show_shortcuts;
        egui::Window::new("단축키").open(&mut shortcuts).resizable(false).show(ctx, |ui| {
            egui::Grid::new("shortcut_grid").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                for (keys, what) in SHORTCUTS {
                    ui.label(RichText::new(*keys).monospace());
                    ui.label(*what);
                    ui.end_row();
                }
            });
        });
        self.show_shortcuts = shortcuts;
    }

    fn play(&mut self, ctx: &egui::Context) {
        if !self.playing {
            return;
        }
        let period = 0.8 / self.speed.max(0.1) as f64;
        if self.now - self.play_at >= period {
            self.play_at = self.now;
            if self.sel.step + 1 < self.cache.steps.len() {
                self.sel.step += 1;
            } else {
                self.playing = false;
            }
        }
        ctx.request_repaint_after(Duration::from_millis(40));
    }

    fn results(&mut self, ui: &mut egui::Ui) {
        self.refresh();
        let mut acts = Vec::new();
        views::draw(self, ui, &mut acts);
        let ctx = ui.ctx().clone();
        for act in acts {
            self.apply(act, &ctx);
        }
    }
}

impl eframe::App for IdeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.now = ctx.input(|i| i.time);
        self.poll();
        if self.auto.is_some() {
            auto::tick(self, ctx);
        }
        self.shortcuts(ctx);
        self.top_bar(ctx);
        self.status_bar(ctx);
        match self.layout {
            Layout::Edit => {
                egui::CentralPanel::default().show(ctx, |ui| editor::draw(self, ui));
            }
            Layout::Results => {
                egui::CentralPanel::default().show(ctx, |ui| self.results(ui));
            }
            Layout::Split => {
                let width = ctx.content_rect().width();
                egui::SidePanel::left("editor_panel")
                    .resizable(true)
                    .default_width(width * 0.42)
                    .width_range(300.0..=(width - 420.0).max(320.0))
                    .show(ctx, |ui| editor::draw(self, ui));
                egui::CentralPanel::default().show(ctx, |ui| self.results(ui));
            }
        }
        self.windows(ctx);
        self.play(ctx);
        self.schedule_check(ctx);
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && self.dirty() && self.auto.is_none() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.may_discard() {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}
