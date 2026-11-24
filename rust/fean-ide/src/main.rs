//! `fean-ide`: the FEAN IDE (Map unit I-11-a).
//!
//!     fean-ide [FILE]          open the window (with a model text, if given)
//!     fean-ide --shots DIR     run the built-in scenario, write screenshots and report.json, quit
//!     fean-ide --features      print the feature list as JSON
//!
//! One window, two halves: a code editor, and the results of running the text (queries with their
//! grounds, contradictions, the transaction graph, the timeline with replay, each actor's ledger
//! and knowledge, the histories, the compiled image). Edit, run, look, click back into the code.

mod app;
mod auto;
mod editor;
mod views;

use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};

use eframe::egui;

/// The exit code of the automatic mode (0 when the scenario ran to its end).
pub static EXIT: AtomicI32 = AtomicI32::new(0);

const HELP: &str = "usage: fean-ide [FILE] | --shots DIR | --features

FEAN IDE: 코드 편집기와 실행 결과 보기.

  FILE            모델 글(.fean)을 열고 시작한다
  --shots DIR     자동 실행 모드: 정해진 시나리오를 수행하며 DIR에 화면 PNG와 report.json을 쓰고 끝낸다
  --features      기능 목록을 JSON으로 출력한다";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut file, mut shots) = (None, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--features" => {
                println!("{}", app::features_json());
                return;
            }
            "-h" | "--help" => {
                println!("{HELP}");
                return;
            }
            "--shots" => {
                i += 1;
                match args.get(i) {
                    Some(dir) => shots = Some(PathBuf::from(dir)),
                    None => {
                        eprintln!("{HELP}\nfean-ide: --shots 에는 폴더가 필요합니다");
                        std::process::exit(3);
                    }
                }
            }
            other if other.starts_with('-') => {
                eprintln!("{HELP}\nfean-ide: 알 수 없는 옵션 {other}");
                std::process::exit(3);
            }
            other => file = Some(PathBuf::from(other)),
        }
        i += 1;
    }
    let size = if shots.is_some() { auto::WINDOW } else { [1380.0, 880.0] };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("FEAN IDE")
            .with_inner_size(size)
            .with_min_inner_size([900.0, 560.0])
            .with_resizable(shots.is_none()),
        centered: true,
        ..Default::default()
    };
    let started = eframe::run_native(
        "FEAN IDE",
        options,
        Box::new(move |cc| {
            app::setup(&cc.egui_ctx);
            Ok(Box::new(app::IdeApp::new(file, shots)))
        }),
    );
    if let Err(e) = started {
        eprintln!("fean-ide: 창을 열 수 없습니다 ({e})");
        std::process::exit(1);
    }
    std::process::exit(EXIT.load(Ordering::SeqCst));
}
