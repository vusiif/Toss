use std::io::{self, Write};
use std::thread;
use std::time::Duration;

use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    println!("{}", style("==================================================================").cyan().bold());
    println!("{}", style("       🦀 Indicatif 进度条与加载动画全家桶全景对比展示 🦀       ").yellow().bold());
    println!("{}", style("==================================================================").cyan().bold());
    println!("本程序将现场在终端运行各类常见进度条，并附上对应的 Rust 代码模板供您直接选用。\n");

    if args.len() > 1 && args[1] == "all" {
        run_all();
        return;
    }

    println!("{}", style("请选择展示模式：").green().bold());
    println!("  [1] 一键连续演示所有风格 (推荐，完整对比)");
    println!("  [2] 经典与 ASCII 风格进度条 (ASCII / Dots / Slim)");
    println!("  [3] 高精度平滑微步 Unicode 进度条 (Smooth Block / Shades / Vertical)");
    println!("  [4] 下载与网络传输进度条 (文件大小 / 传输速度 / 剩余时间 ETA)");
    println!("  [5] 各种 Spinner 加载动画 (Dots / Moon / Clock / Bounce / Line)");
    println!("  [6] 多任务并发进度条 (MultiProgress 同时下载多文件)");
    println!("  [7] 仿工业级 CLI 风格 (Cargo 编译 / Yarn 安装)");
    println!("  [0] 退出");
    print!("\n请输入数字 (默认直接回车运行 [1]): ");
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    let choice = input.trim();

    match choice {
        "" | "1" => run_all(),
        "2" => demo_classic_bars(),
        "3" => demo_fine_bars(),
        "4" => demo_download_bars(),
        "5" => demo_spinners(),
        "6" => demo_multi_bars(),
        "7" => demo_cli_styles(),
        "0" => println!("退出演示。"),
        _ => {
            println!("未知选择，将演示全部风格：");
            run_all();
        }
    }
}

fn run_all() {
    demo_classic_bars();
    pause_briefly();
    demo_fine_bars();
    pause_briefly();
    demo_download_bars();
    pause_briefly();
    demo_spinners();
    pause_briefly();
    demo_multi_bars();
    pause_briefly();
    demo_cli_styles();

    println!("\n{}", style("🎉 全部进度条风格已演示完毕！").green().bold());
    println!("您可以挑选最心仪的风格，复制对应代码段到您的 Rust 项目中。");
}

fn pause_briefly() {
    thread::sleep(Duration::from_millis(600));
}

/// 1. 经典与 ASCII 风格进度条
fn demo_classic_bars() {
    println!("\n{}", style("━━━━━━━━━━━━━━━━ 1. 经典与 ASCII 风格进度条 ━━━━━━━━━━━━━━━━").cyan().bold());

    // 1.1 经典 Cargo 箭头风格
    println!("\n{}", style("【1.1 经典双线箭头风格 (Cargo 风格)】").yellow().bold());
    println!("  代码：.progress_chars(\"=> \")");
    println!("  模板：\"{{prefix:.bold}} [{{bar:40.cyan/blue}}] {{pos}}/{{len}} ({{percent}}%)\"");
    let pb = ProgressBar::new(100);
    pb.set_style(
        ProgressStyle::with_template("{prefix:.bold} [{bar:40.cyan/blue}] {pos}/{len} ({percent}%)")
            .unwrap()
            .progress_chars("=> "),
    );
    pb.set_prefix("Building");
    for _ in 0..100 {
        pb.inc(1);
        thread::sleep(Duration::from_millis(15));
    }
    pb.finish_with_message("Done");

    // 1.2 点状风格 (Dots)
    println!("\n{}", style("【1.2 现代点状风格 (Dot Bar)】").yellow().bold());
    println!("  代码：.progress_chars(\"•- \")");
    let pb = ProgressBar::new(100);
    pb.set_style(
        ProgressStyle::with_template("{prefix:.magenta} [{bar:40.magenta/black}] {pos:>3}/{len:3} [{elapsed_precise}]")
            .unwrap()
            .progress_chars("•- "),
    );
    pb.set_prefix("Processing");
    for _ in 0..100 {
        pb.inc(1);
        thread::sleep(Duration::from_millis(15));
    }
    pb.finish();

    // 1.3 现代细线风格 (Slim Modern)
    println!("\n{}", style("【1.3 现代极简细线风格 (Slim Modern)】").yellow().bold());
    println!("  代码：.progress_chars(\"━┈ \")");
    let pb = ProgressBar::new(100);
    pb.set_style(
        ProgressStyle::with_template("{prefix:.green.bold} ╢{bar:40.green/white}╟ {percent}%")
            .unwrap()
            .progress_chars("━┈ "),
    );
    pb.set_prefix("Syncing");
    for _ in 0..100 {
        pb.inc(1);
        thread::sleep(Duration::from_millis(15));
    }
    pb.finish();
}

/// 2. 高精度平滑微步 Unicode 进度条
fn demo_fine_bars() {
    println!("\n{}", style("━━━━━━━━━━━━ 2. 高精度平滑微步 Unicode 进度条 ━━━━━━━━━━━━").cyan().bold());
    println!("说明：利用 Unicode 1/8 宽度字符，每进一格有 8 级平滑微步动画，视觉极为细腻。");

    let styles = [
        ("【2.1 极致平滑微步块】", "Fine Bar: ", "█▉▊▋▌▍▎▏ ", "green"),
        ("【2.2 垂直高度音波块】", "Vertical: ", "█▇▆▅▄▃▂  ", "yellow"),
        ("【2.3 像素点阵遮罩块】", "Dithered: ", "█▓▒░ ", "magenta"),
        ("【2.4 现代粗实方块】  ", "Blocky:   ", "██ ", "cyan"),
    ];

    let m = MultiProgress::new();
    let mut handles = vec![];

    for (title, prefix, chars, color) in styles {
        let pb = m.add(ProgressBar::new(200));
        let template = format!("{{prefix:.bold}}▕{{bar:36.{}}}▏ {{percent:>3}}%", color);
        pb.set_style(
            ProgressStyle::with_template(&template)
                .unwrap()
                .progress_chars(chars),
        );
        pb.set_prefix(prefix);
        let h = thread::spawn(move || {
            for _ in 0..200 {
                pb.inc(1);
                thread::sleep(Duration::from_millis(10));
            }
            pb.finish();
        });
        handles.push((title, h));
    }

    for (title, h) in handles {
        h.join().unwrap();
        println!("  {} 对应字符：{}", style(title).dim(), styles.iter().find(|s| s.0 == title).unwrap().2);
    }
}

/// 3. 下载与网络传输进度条
fn demo_download_bars() {
    println!("\n{}", style("━━━━━━━━━━━━━━━ 3. 下载与网络传输进度条 ━━━━━━━━━━━━━━━").cyan().bold());

    // 3.1 完整信息下载条 (带旋转、速率、已耗时、预计剩余时间)
    println!("\n{}", style("【3.1 经典全信息下载条 (带 Spinner + 字节 + 速率 + ETA)】").yellow().bold());
    println!("  模板：\"{{spinner:.green}} [{{elapsed_precise}}] [{{bar:35.cyan/blue}}] {{bytes}}/{{total_bytes}} ({{bytes_per_sec}}, {{eta}})\"");
    let total_bytes = 100 * 1024 * 1024; // 100 MB
    let pb = ProgressBar::new(total_bytes);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:35.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})",
        )
        .unwrap()
        .progress_chars("━╸ "),
    );

    let mut current = 0;
    while current < total_bytes {
        let chunk = 2 * 1024 * 1024;
        current += chunk;
        pb.set_position(current);
        thread::sleep(Duration::from_millis(25));
    }
    pb.finish_with_message("Download Complete");

    // 3.2 紧凑现代下载条
    println!("\n{}", style("【3.2 紧凑型现代下载条】").yellow().bold());
    println!("  模板：\"{{msg}} [{{bar:25.yellow/white}}] {{percent}}% ({{bytes}}/{{total_bytes}})\"");
    let pb2 = ProgressBar::new(total_bytes);
    pb2.set_style(
        ProgressStyle::with_template("{msg:.bold} [{bar:25.yellow/white}] {percent:>3}% ({bytes}/{total_bytes})")
            .unwrap()
            .progress_chars("■□ "),
    );
    pb2.set_message("Fetching runtime");
    let mut current2 = 0;
    while current2 < total_bytes {
        current2 += 3 * 1024 * 1024;
        pb2.set_position(current2);
        thread::sleep(Duration::from_millis(20));
    }
    pb2.finish_with_message("Done");
}

/// 4. 各种 Spinner 加载动画
fn demo_spinners() {
    println!("\n{}", style("━━━━━━━━━━━━━━━━━ 4. 各种 Spinner 加载动画 ━━━━━━━━━━━━━━━━━").cyan().bold());
    println!("用于未知总任务长度的后台轮询、网络等待、编译等场景：\n");

    let spinner_types = [
        ("【4.1 经典盲文圆点 (Dots)】", "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏", "Connecting to server..."),
        ("【4.2 月相盈亏 (Moon)】     ", "🌑🌒🌓🌔🌕🌖🌗🌘", "Syncing ledger blocks..."),
        ("【4.3 走动的时钟 (Clock)】   ", "🕐🕑🕒🕓🕔🕕🕖🕗🕘🕙🕚🕛", "Waiting for lock release..."),
        ("【4.4 经典终端斜杠 (Line)】   ", "|/-\\", "Compiling native module..."),
        ("【4.5 脉冲方块 (Bouncing)】   ", "▖▘▝▗", "Optimizing bytecode..."),
        ("【4.6 弧线旋转 (Arc)】        ", "◜◠◝◞◡◟", "Cleaning cache directory..."),
    ];

    let m = MultiProgress::new();
    let mut handles = vec![];

    for (_name, chars, msg) in spinner_types {
        let pb = m.add(ProgressBar::new_spinner());
        pb.set_style(
            ProgressStyle::with_template("{spinner:.green.bold} {msg}")
                .unwrap()
                .tick_chars(chars),
        );
        pb.set_message(msg);
        pb.enable_steady_tick(Duration::from_millis(80));

        let h = thread::spawn(move || {
            thread::sleep(Duration::from_millis(1800));
            pb.finish_with_message(format!("{} {}", style("✔").green().bold(), msg));
        });
        handles.push(h);
    }

    for h in handles {
        h.join().unwrap();
    }
}

/// 5. 多任务并发进度条 (MultiProgress)
fn demo_multi_bars() {
    println!("\n{}", style("━━━━━━━━━━━━━━━ 5. 多任务并发进度条 (MultiProgress) ━━━━━━━━━━━━━━━").cyan().bold());
    println!("模拟并行下载 3 个依赖包或多线程处理数据：\n");

    let m = MultiProgress::new();
    let tasks = [
        ("tokio-v1.40.0.tar.gz    ", 80, "cyan"),
        ("serde_json-v1.0.128.crate", 120, "green"),
        ("linux-kernel-header.pkg ", 160, "yellow"),
    ];

    let mut handles = vec![];

    for (name, len, color) in tasks {
        let pb = m.add(ProgressBar::new(len));
        let tmpl = format!("{{prefix:.bold}} [{{bar:30.{}}}] {{percent:>3}}% ({{eta}})", color);
        pb.set_style(
            ProgressStyle::with_template(&tmpl)
                .unwrap()
                .progress_chars("█▉▊▋▌▍▎▏ "),
        );
        pb.set_prefix(name);

        let h = thread::spawn(move || {
            for _ in 0..len {
                pb.inc(1);
                thread::sleep(Duration::from_millis(15));
            }
            pb.finish_with_message("Finished");
        });
        handles.push(h);
    }

    for h in handles {
        h.join().unwrap();
    }
}

/// 6. 仿工业级 CLI 风格 (Cargo / Yarn)
fn demo_cli_styles() {
    println!("\n{}", style("━━━━━━━━━━━━━━ 6. 仿工业级 CLI 风格 (Cargo / Yarn) ━━━━━━━━━━━━━━").cyan().bold());

    // 6.1 仿 Yarn / NPM 步骤风格
    println!("\n{}", style("【6.1 仿 Yarn / PNPM 多阶段安装风格】").yellow().bold());
    let steps = [
        ("[1/4]", "🔍 Resolving dependencies..."),
        ("[2/4]", "🚚 Fetching packages from registry..."),
        ("[3/4]", "🔗 Linking dependencies to node_modules..."),
        ("[4/4]", "🔨 Running post-install build scripts..."),
    ];

    for (num, desc) in steps {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{prefix:.bold.cyan} {spinner:.yellow} {msg}")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
        );
        pb.set_prefix(num);
        pb.set_message(desc);
        pb.enable_steady_tick(Duration::from_millis(60));
        thread::sleep(Duration::from_millis(500));
        pb.finish_with_message(format!("{} {}", style("✔").green().bold(), desc));
    }

    // 6.2 仿 Cargo 编译条
    println!("\n{}", style("【6.2 仿 Cargo 编译动态行】").yellow().bold());
    let pb = ProgressBar::new(40);
    pb.set_style(
        ProgressStyle::with_template("{prefix:>12.green.bold} [{bar:40.cyan}] {pos:>2}/{len:2}: {msg}")
            .unwrap()
            .progress_chars("=> "),
    );
    pb.set_prefix("Compiling");
    let crates = ["libc", "syn", "serde", "tokio", "indicatif", "actix-web", "tracing", "sqlx"];
    for (i, c) in crates.iter().enumerate() {
        pb.set_message(format!("{}-v1.{}.0", c, i + 1));
        pb.inc(5);
        thread::sleep(Duration::from_millis(120));
    }
    pb.finish_and_clear();
    println!("{:>12} dev [unoptimized + debuginfo] target(s) in 0.96s", style("Finished").green().bold());
}
