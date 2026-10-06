//! VM-only acceptance harness. Never installed into a system release.
use anyhow::{Context, Result, ensure};
use looom::{
    credentials::{self, Accounts},
    machine::{Machine, STATE},
    util::*,
};
use std::{
    ffi::CString,
    fs::{self, File},
    io::Read,
    path::Path,
};
use std::{
    io::Write,
    os::fd::AsRawFd,
    os::unix::fs::OpenOptionsExt,
    process::{Command, Stdio},
    time::Duration,
};
use zeroize::Zeroizing;

#[repr(C)]
struct InputId {
    bus: u16,
    vendor: u16,
    product: u16,
    version: u16,
}
#[repr(C)]
struct KeyboardSetup {
    id: InputId,
    name: [libc::c_char; 80],
    ff_effects_max: u32,
}
struct Keyboard(File);
impl Drop for Keyboard {
    fn drop(&mut self) {
        unsafe {
            libc::ioctl(self.0.as_raw_fd(), 0x5502 as libc::c_ulong);
        }
    }
}
impl Keyboard {
    fn event(&mut self, kind: u16, code: u16, value: i32) -> Result<()> {
        let event = libc::input_event {
            time: libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
            type_: kind,
            code,
            value,
        };
        let bytes = unsafe {
            std::slice::from_raw_parts(
                std::ptr::from_ref(&event).cast::<u8>(),
                std::mem::size_of_val(&event),
            )
        };
        self.0.write_all(bytes)?;
        Ok(())
    }
    fn key(&mut self, codes: &[u16]) -> Result<()> {
        for code in codes {
            self.event(1, *code, 1)?;
        }
        self.event(0, 0, 0)?;
        std::thread::sleep(Duration::from_millis(35));
        for code in codes.iter().rev() {
            self.event(1, *code, 0)?;
        }
        self.event(0, 0, 0)?;
        std::thread::sleep(Duration::from_millis(35));
        Ok(())
    }
}
fn gui_login(password: &str) -> Result<()> {
    ensure!(
        password
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "GUI harness accepts only guest-generated lowercase hex password"
    );
    command("modprobe", &["uinput"])?;
    let file = fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open("/dev/uinput")?;
    unsafe {
        for event in [0, 1] {
            ensure!(
                libc::ioctl(file.as_raw_fd(), 0x40045564 as libc::c_ulong, event) == 0,
                "cannot configure VM input events"
            );
        }
        for code in 0..256 {
            ensure!(
                libc::ioctl(file.as_raw_fd(), 0x40045565 as libc::c_ulong, code) == 0,
                "cannot configure VM keyboard keys"
            );
        }
        let mut setup = KeyboardSetup {
            id: InputId {
                bus: 3,
                vendor: 0xc0de,
                product: 0xcafe,
                version: 1,
            },
            name: [0; 80],
            ff_effects_max: 0,
        };
        for (slot, byte) in setup.name.iter_mut().zip(b"looom VM acceptance keyboard") {
            *slot = *byte as libc::c_char;
        }
        let ioctl = (1u64 << 30)
            | ((std::mem::size_of::<KeyboardSetup>() as u64) << 16)
            | (u64::from(b'U') << 8)
            | 3;
        ensure!(
            libc::ioctl(
                file.as_raw_fd(),
                ioctl as libc::c_ulong,
                std::ptr::from_ref(&setup)
            ) == 0,
            "cannot set up VM keyboard"
        );
        ensure!(
            libc::ioctl(file.as_raw_fd(), 0x5501 as libc::c_ulong) == 0,
            "cannot create VM keyboard"
        );
    }
    let mut keyboard = Keyboard(file);
    std::thread::sleep(Duration::from_secs(2));
    keyboard.key(&[29, 30])?;
    keyboard.key(&[14])?;
    for byte in password.bytes() {
        let code = match byte {
            b'1'..=b'9' => u16::from(byte - b'1') + 2,
            b'0' => 11,
            b'a' => 30,
            b'b' => 48,
            b'c' => 46,
            b'd' => 32,
            b'e' => 18,
            b'f' => 33,
            _ => unreachable!(),
        };
        keyboard.key(&[code])?;
    }
    keyboard.key(&[28])?;
    std::thread::sleep(Duration::from_secs(1));
    println!("Guest temporary password entered through guest uinput; verify the actual session");
    Ok(())
}

#[repr(C)]
struct Message {
    style: libc::c_int,
    text: *const libc::c_char,
}
#[repr(C)]
struct Response {
    answer: *mut libc::c_char,
    status: libc::c_int,
}
struct Answers {
    password: Zeroizing<Vec<u8>>,
    user: CString,
}
type Callback = unsafe extern "C" fn(
    libc::c_int,
    *mut *const Message,
    *mut *mut Response,
    *mut libc::c_void,
) -> libc::c_int;
#[repr(C)]
struct Conversation {
    function: Callback,
    data: *mut libc::c_void,
}
unsafe extern "C" fn converse(
    count: libc::c_int,
    messages: *mut *const Message,
    result: *mut *mut Response,
    data: *mut libc::c_void,
) -> libc::c_int {
    if !(1..=32).contains(&count) || messages.is_null() || result.is_null() || data.is_null() {
        return 19;
    }
    unsafe {
        let answers = &*(data.cast::<Answers>());
        let allocation =
            libc::calloc(count as usize, std::mem::size_of::<Response>()).cast::<Response>();
        if allocation.is_null() {
            return 19;
        }
        for index in 0..count as usize {
            let message = *messages.add(index);
            if message.is_null() {
                libc::free(allocation.cast());
                return 19;
            }
            let answer = match (*message).style {
                1 => answers.password.as_ptr().cast(),
                2 => answers.user.as_ptr(),
                3 | 4 => std::ptr::null(),
                _ => {
                    libc::free(allocation.cast());
                    return 19;
                }
            };
            if !answer.is_null() {
                (*allocation.add(index)).answer = libc::strdup(answer);
            }
        }
        *result = allocation;
    }
    0
}
fn authenticate(user: &str, password: &str, service: &str) -> Result<bool> {
    let user = CString::new(user)?;
    let service = CString::new(service)?;
    let mut password = Zeroizing::new(password.as_bytes().to_vec());
    password.push(0);
    let mut answers = Answers { password, user };
    let conversation = Conversation {
        function: converse,
        data: std::ptr::from_mut(&mut answers).cast(),
    };
    unsafe {
        let lib = libloading::Library::new("libpam.so.0")?;
        let start: libloading::Symbol<
            unsafe extern "C" fn(
                *const libc::c_char,
                *const libc::c_char,
                *const Conversation,
                *mut *mut libc::c_void,
            ) -> libc::c_int,
        > = lib.get(b"pam_start")?;
        let auth: libloading::Symbol<
            unsafe extern "C" fn(*mut libc::c_void, libc::c_int) -> libc::c_int,
        > = lib.get(b"pam_authenticate")?;
        let account: libloading::Symbol<
            unsafe extern "C" fn(*mut libc::c_void, libc::c_int) -> libc::c_int,
        > = lib.get(b"pam_acct_mgmt")?;
        let end: libloading::Symbol<
            unsafe extern "C" fn(*mut libc::c_void, libc::c_int) -> libc::c_int,
        > = lib.get(b"pam_end")?;
        let mut handle = std::ptr::null_mut();
        ensure!(
            start(
                service.as_ptr(),
                answers.user.as_ptr(),
                &conversation,
                &mut handle
            ) == 0,
            "PAM start failed"
        );
        let mut status = auth(handle, 0);
        if status == 0 {
            status = account(handle, 0);
        }
        end(handle, status);
        Ok(status == 0)
    }
}
fn input_failures() -> Result<()> {
    let source = Path::new(STATE).join("dev/src/configs/native/b");
    let fixture = tempfile::Builder::new()
        .prefix("native-inputs-")
        .tempdir_in(Path::new(STATE).join("dev"))?;
    let yaml = fixture.path().join("base.yaml");
    fs::copy(source.join("base.yaml"), &yaml)?;
    let (cfg, _) = looom::config::load(&yaml)?;
    let mut locked: looom::packages::PackageLock =
        serde_json::from_slice(&fs::read(source.join("base.lock"))?)?;
    locked.validate(&cfg, true)?;
    locked.request["recipe_sha256"] = hash_file(Path::new("/usr/bin/looom"))?.into();
    json(&fixture.path().join("base.lock"), &locked, 0o600)?;
    let invoke = |operation: &str, id: Option<&str>| -> Result<std::process::Output> {
        let mut command = Command::new("/usr/bin/looom");
        command.arg(operation).arg(&yaml);
        if let Some(id) = id {
            command.arg(id);
        }
        Ok(command.stdin(Stdio::null()).output()?)
    };
    ensure!(
        invoke("plan", None)?.status.success(),
        "valid input lock rejected"
    );
    locked.request["recipe_sha256"] = "0".repeat(64).into();
    json(&fixture.path().join("base.lock"), &locked, 0o600)?;
    let stale = invoke("plan", None)?;
    ensure!(
        !stale.status.success()
            && String::from_utf8_lossy(&stale.stderr).contains("does not match"),
        "stale recipe lock accepted or wrong failure"
    );
    locked.request["recipe_sha256"] = hash_file(Path::new("/usr/bin/looom"))?.into();
    locked.packages[0].sha256 = "0".repeat(64);
    json(&fixture.path().join("base.lock"), &locked, 0o600)?;
    let id = "rust-input-negative";
    for path in [
        Path::new(STATE).join("inputs").join(id),
        Path::new(STATE).join("releases").join(format!("{id}.json")),
    ] {
        ensure!(!path.exists(), "negative test ID already used");
    }
    let corrupt = invoke("build", Some(id))?;
    ensure!(
        !corrupt.status.success()
            && String::from_utf8_lossy(&corrupt.stderr).contains("hash mismatch"),
        "corrupt archive digest accepted or wrong failure"
    );
    ensure!(
        !Path::new(STATE).join("inputs").join(id).exists(),
        "corrupt lock created frozen build inputs"
    );
    println!(
        "PASS: installed Rust CLI accepts valid lock, rejects stale recipe and corrupted archive digest before creating build inputs"
    );
    Ok(())
}
fn desktop(machine: &Machine) -> Result<()> {
    let sessions = output("loginctl", &["list-sessions", "--no-legend"])?;
    let mut verified = None;
    for line in sessions.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 3 || fields[2] != machine.user {
            continue;
        }
        let session = output(
            "loginctl",
            &[
                "show-session",
                fields[0],
                "-p",
                "Name",
                "-p",
                "User",
                "-p",
                "Type",
                "-p",
                "Active",
                "-p",
                "Seat",
            ],
        )?;
        if session.lines().any(|s| s == "Type=wayland")
            && session.lines().any(|s| s == "Active=yes")
            && session.lines().any(|s| s == "Seat=seat0")
        {
            verified = Some(session);
            break;
        }
    }
    println!(
        "{}",
        verified.context("no active local managed-user Wayland session")?
    );
    let user_machine = format!("{}@.host", machine.user);
    for unit in [
        "plasma-plasmashell.service",
        "plasma-kwin_wayland.service",
        "pipewire.service",
        "pipewire-pulse.service",
        "wireplumber.service",
    ] {
        ensure!(
            output(
                "systemctl",
                &["--user", "-M", &user_machine, "is-active", unit]
            )? == "active",
            "desktop unit is not active: {unit}"
        );
        println!("{unit}: active");
    }
    ensure!(
        output(
            "systemctl",
            &[
                "--user",
                "-M",
                &user_machine,
                "--failed",
                "--no-legend",
                "--plain"
            ]
        )?
        .is_empty(),
        "failed user services"
    );
    println!("PASS: actual local Plasma Wayland session and audio services");
    Ok(())
}
fn report(machine: &Machine) -> Result<()> {
    desktop(machine)?;
    ensure!(
        !Path::new(STATE).join("private-password-test").exists(),
        "temporary password state must be restored before final report"
    );
    let runtime = credentials::read_private(Path::new("/run/looom/accounts/shadow"))?;
    for user in ["root", machine.user.as_str()] {
        let persistent = credentials::read_private(
            &Path::new(STATE)
                .join("credentials")
                .join(format!("{user}.hash")),
        )?;
        ensure!(
            runtime
                .lines()
                .find(|l| l.split(':').next() == Some(user))
                .and_then(|l| l.split(':').nth(1))
                == Some(persistent.trim()),
            "runtime and persistent credential differ"
        );
    }
    let manager = looom::releases::Manager::installed(machine.clone())?;
    let current = fs::read_to_string("/etc/looom/release-id")?
        .trim()
        .to_owned();
    let metadata = manager.load(&current)?;
    ensure!(
        metadata.phase == "confirmed",
        "final release is not confirmed"
    );
    manager.health(&current)?;
    let environment = output("grub-editenv", &["/efi/looom/grub/grubenv", "list"])?;
    ensure!(
        environment
            .lines()
            .any(|l| l == format!("saved_entry=looom-{current}"))
            && !environment
                .lines()
                .any(|l| l.starts_with("next_entry=") && l != "next_entry="),
        "final boot choice not settled"
    );
    let bundle: looom::builder::Bundle =
        serde_json::from_slice(&fs::read("/usr/lib/looom/declaration.json")?)?;
    let lock = bundle.lock;
    ensure!(
        looom::packages::inventory(None)? == lock.inventory(),
        "final inventory differs"
    );
    let mut source = std::collections::BTreeMap::new();
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "src/lib.rs",
        "src/main.rs",
        "src/config.rs",
        "src/util.rs",
        "src/machine.rs",
        "src/credentials.rs",
        "src/packages.rs",
        "src/releases.rs",
        "src/builder.rs",
        "src/bootstrap.rs",
        "configs/bootstrap/packages.txt",
    ] {
        let installed = Path::new("/usr/lib/looom/source").join(name);
        ensure!(
            fs::read(&installed)? == fs::read(Path::new(STATE).join("dev/src").join(name))?,
            "installed and recorded source differ: {name}"
        );
        source.insert(name, hash_file(&installed)?);
    }
    let value = serde_json::json!({"schema":1,"manager":"native Rust 0.2.0","release":metadata,"machine":machine,"kernel":output("uname", &["-r"])? ,"package_count":lock.packages.len(),"manager_sha256":hash_file(Path::new("/usr/bin/looom"))?,"grub_environment":environment,"firmware":output("efibootmgr", &[])?,"failed_system_units":output("systemctl", &["--failed", "--no-legend", "--plain"])? ,"password_restored":true,"runtime_credentials_match":true,"source_sha256":source});
    json(
        &Path::new(STATE).join("dev/evidence/native-final-state.json"),
        &value,
        0o600,
    )?;
    println!(
        "PASS: confirmed final state, original password restored, runtime reconciled, exact package inventory and frozen source recorded without secrets"
    );
    Ok(())
}
fn run() -> Result<()> {
    root()?;
    credentials::disable_dumps()?;
    ensure!(
        matches!(
            output("systemd-detect-virt", &["--vm"])?.as_str(),
            "qemu" | "kvm"
        ),
        "VM harness only"
    );
    ensure!(
        fs::read_to_string("/sys/class/net/enp1s0/address")?.trim() == "52:54:00:7b:23:63",
        "unexpected VM identity"
    );
    let args: Vec<String> = std::env::args().skip(1).collect();
    let operation = args
        .first()
        .context("prepare|verify|restore [PAM service]")?;
    if operation == "inputs" {
        return input_failures();
    }
    let machine = Machine::load()?;
    if operation == "desktop" {
        return desktop(&machine);
    }
    if operation == "report" {
        return report(&machine);
    }
    let accounts = Accounts::installed(&machine);
    let test = Path::new(STATE).join("private-password-test");
    let hash_path = accounts.credentials.join(format!("{}.hash", machine.user));
    match operation.as_str() {
        "prepare" => {
            ensure!(!test.exists(), "restore existing password test first");
            mkdir(&test, 0o700)?;
            let original = credentials::read_private(&hash_path)?;
            credentials::private_atomic(&test.join("original.hash"), original.as_bytes())?;
            let mut entropy = [0u8; 32];
            File::open("/dev/urandom")?.read_exact(&mut entropy)?;
            let password = Zeroizing::new(hash(&entropy));
            credentials::private_atomic(&test.join("password.secret"), password.as_bytes())?;
            accounts.set_password(&machine.user, &password)?;
            println!("Guest temporary password changed through native Rust credentials");
        }
        "restore" => {
            let original = credentials::read_private(&test.join("original.hash"))?;
            credentials::validate_hash(original.trim())?;
            credentials::private_atomic(&hash_path, original.as_bytes())?;
            accounts.generate()?;
            ensure!(
                *credentials::read_private(&hash_path)? == *original,
                "restored credential differs"
            );
            fs::remove_file(test.join("password.secret"))?;
            fs::remove_file(test.join("original.hash"))?;
            fs::remove_dir(&test)?;
            println!("Original user password restored; protected temporary test state removed");
            return Ok(());
        }
        "verify" | "gui-login" | "cli-change" => {}
        _ => anyhow::bail!("unknown VM harness operation"),
    }
    let password = credentials::read_private(&test.join("password.secret"))?;
    if operation == "gui-login" {
        return gui_login(&password);
    }
    if operation == "cli-change" {
        let mut child = Command::new("/usr/bin/looom-password")
            .args([machine.user.as_str(), "--stdin"])
            .stdin(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .context("password stdin")?
            .write_all(password.as_bytes())?;
        ensure!(
            child.wait()?.success(),
            "installed native password CLI failed"
        );
        println!("PASS: installed looom-password alias and protected stdin interface");
    }
    let service = args.get(1).map_or("login", String::as_str);
    ensure!(
        ["login", "sddm", "sudo"].contains(&service),
        "unsupported test PAM service"
    );
    ensure!(
        authenticate(&machine.user, &password, service)?,
        "correct guest password rejected"
    );
    let wrong = Zeroizing::new(format!("{}-wrong", password.as_str()));
    ensure!(
        !authenticate(&machine.user, &wrong, service)?,
        "incorrect guest password accepted"
    );
    println!("PASS: {service} PAM accepts persistent password and rejects incorrect password");
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("VM acceptance: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
