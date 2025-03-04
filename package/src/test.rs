// Copyright 2023 Pants project contributors.
// Licensed under the Apache License, Version 2.0 (see LICENSE).

use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result};
use tempfile::TempDir;
use termcolor::{Color, WriteColor};

use crate::utils::build::fingerprint;
use crate::utils::exe::{execute, execute_with_input, Platform, CURRENT_PLATFORM};
use crate::utils::fs::{
    copy, create_tempdir, ensure_directory, remove_dir, rename, softlink, touch, write_file,
};
use crate::utils::os::{EOL, PATHSEP};
use crate::{build_step, log};

macro_rules! integration_test {
    ($msg:expr $(,)?) => {
        log!(::termcolor::Color::Magenta, ">> {}", format!($msg));
    };
    ($msg:expr $(,)?, $($arg:tt)*) => {
        log!(::termcolor::Color::Magenta, ">> {}", format!($msg, $($arg)*));
    };
}

fn issue_link(issue: usize) -> String {
    format!("https://github.com/pantsbuild/scie-pants/issues/{issue}")
}

fn decode_output(output: Vec<u8>) -> Result<String> {
    String::from_utf8(output).context("Failed to decode Pants output.")
}

fn assert_stderr_output(command: &mut Command, expected_messages: Vec<&str>) -> Output {
    let output = execute(command.stderr(Stdio::piped())).unwrap();
    let stderr = decode_output(output.stderr.clone()).unwrap();
    for expected_message in expected_messages {
        assert!(
            stderr.contains(expected_message),
            "STDERR did not contain '{expected_message}':\n{stderr}"
        );
    }
    output
}

pub(crate) fn run_integration_tests(
    workspace_root: &Path,
    tools_pex_path: &Path,
    scie_pants_scie: &Path,
    check: bool,
    tools_pex_mismatch_warn: bool,
) -> Result<()> {
    build_step!("Running smoke tests");
    log!(
        Color::Yellow,
        "Disabling pants rc files for the smoke tests."
    );
    env::set_var("PANTS_PANTSRC", "False");

    // Our `.pants.bootstrap` uses `tput` which requires TERM be set: ensure it is.
    env::set_var("TERM", env::var_os("TERM").unwrap_or_else(|| "dumb".into()));

    // Max Python supported is 3.9 and only Linux x86_64 and macOS aarch64 and x86_64 wheels were
    // released.
    if matches!(
        *CURRENT_PLATFORM,
        Platform::LinuxX86_64 | Platform::MacOSAarch64 | Platform::MacOSX86_64
    ) {
        test_tools(scie_pants_scie, check);
        test_pants_bin_name_handling(scie_pants_scie);
        test_pants_bootstrap_handling(scie_pants_scie);
        test_tools_pex_reproducibility(workspace_root, tools_pex_path, tools_pex_mismatch_warn);
        test_pants_bootstrap_tools(scie_pants_scie);

        // TODO(John Sirois): The --no-pantsd here works around a fairly prevalent Pants crash on
        // Linux x86_64 along the lines of the following, but sometimes varying:
        // >> Verifying PANTS_SHA is respected
        // Bootstrapping Pants 2.14.0a0+git8e381dbf using cpython 3.9.15
        // Installing pantsbuild.pants==2.14.0a0+git8e381dbf into a virtual environment at /home/runner/.cache/nce/67f27582b3729c677922eb30c5c6e210aa54badc854450e735ef41cf25ac747f/bindings/venvs/2.14.0a0+git8e381dbf
        // New virtual environment successfully created at /home/runner/.cache/nce/67f27582b3729c677922eb30c5c6e210aa54badc854450e735ef41cf25ac747f/bindings/venvs/2.14.0a0+git8e381dbf.
        // 18:11:53.75 [INFO] Initializing scheduler...
        // 18:11:53.97 [INFO] Scheduler initialized.
        // 2.14.0a0+git8e381dbf
        // Fatal Python error: PyGILState_Release: thread state 0x7efe18001140 must be current when releasing
        // Python runtime state: finalizing (tstate=0x1f4b810)
        //
        // Thread 0x00007efe30b75540 (most recent call first):
        // <no Python frame>
        // Error: Command "/home/runner/work/scie-pants/scie-pants/dist/scie-pants-linux-x86_64" "--no-verify-config" "-V" failed with exit code: None
        if matches!(*CURRENT_PLATFORM, Platform::LinuxX86_64) {
            log!(Color::Yellow, "Turning off pantsd for remaining tests.");
            env::set_var("PANTS_PANTSD", "False");
        }

        test_pants_shas(scie_pants_scie);
        test_python_repos_repos(scie_pants_scie);
        test_initialize_new_pants_project(scie_pants_scie);
        test_set_pants_version(scie_pants_scie);

        let clone_root = create_tempdir()?;
        test_use_in_repo_with_pants_script(scie_pants_scie, &clone_root);
        test_dot_env_loading(scie_pants_scie, &clone_root);

        let dev_cache_dir = crate::utils::fs::dev_cache_dir()?;
        let clone_dir = dev_cache_dir.join("clones");
        let pants_2_14_1_clone_dir = clone_dir.join("pants-2.14.1");
        let venv_dir = dev_cache_dir.join("venvs");
        let pants_2_14_1_venv_dir = venv_dir.join("pants-2.14.1");

        test_pants_source_mode(
            scie_pants_scie,
            &clone_dir,
            &pants_2_14_1_clone_dir,
            &venv_dir,
            &pants_2_14_1_venv_dir,
        );
        test_pants_from_sources_mode(
            scie_pants_scie,
            &pants_2_14_1_clone_dir,
            &pants_2_14_1_venv_dir,
        );
        test_delegate_pants_in_pants_repo(scie_pants_scie, &pants_2_14_1_clone_dir);
        test_use_pants_release_in_pants_repo(scie_pants_scie, &pants_2_14_1_clone_dir);

        test_caching_issue_129(scie_pants_scie);
        test_custom_pants_toml_issue_153(scie_pants_scie);
        test_pants_native_client_perms_issue_182(scie_pants_scie);
    }

    // Max Python supported is 3.8 and only Linux and macOS x86_64 wheels were released.
    if matches!(
        *CURRENT_PLATFORM,
        Platform::LinuxX86_64 | Platform::MacOSX86_64
    ) {
        test_python38_used_for_old_pants(scie_pants_scie);
    }

    test_self_update(scie_pants_scie);
    test_self_downgrade(scie_pants_scie);

    Ok(())
}

fn test_tools(scie_pants_scie: &Path, check: bool) {
    integration_test!("Linting, testing and packaging the tools codebase");

    let tput_output = |subcommand| {
        let result = execute(Command::new("tput").arg(subcommand).stdout(Stdio::piped()))
            .unwrap()
            .stdout;
        String::from_utf8(result)
            .with_context(|| format!("Failed to decode output of tput {subcommand} as UTF-*"))
            .unwrap()
    };
    let mut command = Command::new(scie_pants_scie);
    if !check {
        command.arg("fmt");
    }
    execute(
        command
            .args(["lint", "check", "test", "package", "::"])
            .env("PEX_SCRIPT", "Does not exist!")
            .env("EXPECTED_COLUMNS", tput_output("cols").trim())
            .env("EXPECTED_LINES", tput_output("lines").trim()),
    )
    .unwrap();
}

fn test_pants_bin_name_handling(scie_pants_scie: &Path) {
    integration_test!("Checking PANTS_BIN_NAME handling");
    let check_pants_bin_name_chroot = create_tempdir().unwrap();

    let bin_dir = check_pants_bin_name_chroot.path().join("bin");
    let project_dir = check_pants_bin_name_chroot.path().join("project");
    let existing_path =
        env::split_paths(&env::var_os("PATH").unwrap_or("".into())).collect::<Vec<_>>();
    let path = env::join_paths(
        [bin_dir.as_os_str()]
            .into_iter()
            .chain(existing_path.iter().map(|p| p.as_os_str())),
    )
    .unwrap();
    ensure_directory(&bin_dir, true).unwrap();

    ensure_directory(&project_dir, true).unwrap();
    write_file(
        &project_dir.join("pants.toml"),
        false,
        r#"
            [GLOBAL]
            pants_version = "2.15.0rc5"
            [anonymous-telemetry]
            enabled = false
            "#,
    )
    .unwrap();

    softlink(scie_pants_scie, &bin_dir.join("foo")).unwrap();
    softlink(scie_pants_scie, &project_dir.join("bar")).unwrap();
    let absolute_argv0_path = check_pants_bin_name_chroot.path().join("baz");
    softlink(scie_pants_scie, &absolute_argv0_path).unwrap();

    let assert_pants_bin_name = |argv0: &str, expected_bin_name: &str, extra_envs: Vec<(_, _)>| {
        let output = String::from_utf8(
            execute(
                Command::new(argv0)
                    .arg("help-advanced")
                    .arg("global")
                    .env("PATH", &path)
                    .envs(extra_envs)
                    .current_dir(&project_dir)
                    .stdout(Stdio::piped()),
            )
            .unwrap()
            .stdout,
        )
        .unwrap();
        let expected_output =
            format!("current value: {expected_bin_name} (from env var PANTS_BIN_NAME)");
        assert!(
            output.contains(&expected_output),
            "Expected:{EOL}{expected_output}{EOL}STDOUT was:{EOL}{output}",
        );
    };

    assert_pants_bin_name("foo", "foo", vec![]);
    assert_pants_bin_name("./bar", "./bar", vec![]);

    let absolute_argv0 = absolute_argv0_path.to_str().unwrap();
    assert_pants_bin_name(absolute_argv0, absolute_argv0, vec![]);
    assert_pants_bin_name(absolute_argv0, "spam", vec![("PANTS_BIN_NAME", "spam")]);
}

fn test_pants_bootstrap_handling(scie_pants_scie: &Path) {
    integration_test!("Checking .pants.bootstrap handling ignores bash functions");
    // N.B.: We run this test after 1st having run the test above to ensure pants is already
    // bootstrapped so that we don't get stderr output from that process. We also use
    // `--no-pantsd` to avoid spurious pantsd startup stderr log lines just in case pantsd found
    // a need to restart.
    let output = execute(
        Command::new(scie_pants_scie)
            .args(["--no-pantsd", "-V"])
            .stderr(Stdio::piped()),
    )
    .unwrap();
    assert!(
        output.stderr.is_empty(),
        "Expected no warnings to be printed when handling .pants.bootstrap, found:\n{warnings}",
        warnings = String::from_utf8_lossy(&output.stderr)
    );
}

fn test_tools_pex_reproducibility(
    workspace_root: &Path,
    tools_pex_path: &Path,
    tools_pex_mismatch_warn: bool,
) {
    integration_test!(
        "Verifying the tools.pex built by the package crate matches the tools.pex built by \
            Pants"
    );
    let pants_tools_pex_path = workspace_root.join("dist").join("tools").join("tools.pex");
    let pants_tools_pex_fingerprint = fingerprint(&pants_tools_pex_path).unwrap();
    let our_tools_pex_fingerprint = fingerprint(tools_pex_path).unwrap();
    if !tools_pex_mismatch_warn {
        assert_eq!(our_tools_pex_fingerprint, pants_tools_pex_fingerprint);
    } else if our_tools_pex_fingerprint != pants_tools_pex_fingerprint {
        log!(
            Color::Yellow,
            "The tools.pex generated by Pants does not match ours:{eol}\
                Ours:  {our_tools_path}{eol}\
                ->     {ours}{eol}\
                Pants: {pants_tools_path}{eol}\
                ->     {pants}{eol}",
            our_tools_path = tools_pex_path.display(),
            ours = our_tools_pex_fingerprint,
            pants_tools_path = pants_tools_pex_path.display(),
            pants = pants_tools_pex_fingerprint,
            eol = EOL,
        );
    }
}

fn test_pants_bootstrap_tools(scie_pants_scie: &Path) {
    integration_test!("Verifying PANTS_BOOTSTRAP_TOOLS works");
    execute(
        Command::new(scie_pants_scie)
            .env("PANTS_BOOTSTRAP_TOOLS", "1")
            .args(["bootstrap-cache-key"]),
    )
    .unwrap();
}

fn test_pants_shas(scie_pants_scie: &Path) {
    for sha in [
        // initial
        "8e381dbf90cae57c5da2b223c577b36ca86cace9",
        // native-client added to wheel
        "558d843549204bbe49c351d00cdf23402da262c1",
    ] {
        integration_test!("Verifying significant PANTS_SHA: {sha}");
        let existing_project_dir = create_tempdir().unwrap();
        touch(&existing_project_dir.path().join("pants.toml")).unwrap();
        execute(
            Command::new(scie_pants_scie)
                .current_dir(existing_project_dir.path())
                .env("PANTS_SHA", sha)
                .args(["--no-verify-config", "-V"]),
        )
        .unwrap();
    }
}

fn test_python_repos_repos(scie_pants_scie: &Path) {
    integration_test!(
        "Verifying --python-repos-repos is used prior to Pants 2.13 (no warnings should be \
            issued by Pants)"
    );
    execute(
        Command::new(scie_pants_scie)
            .env("PANTS_VERSION", "2.12.1")
            .args(["--no-verify-config", "-V"]),
    )
    .unwrap();
}

fn test_initialize_new_pants_project(scie_pants_scie: &Path) {
    integration_test!("Verifying initializing a new Pants project works");
    let new_project_dir = create_tempdir().unwrap();
    execute(Command::new("git").arg("init").arg(new_project_dir.path())).unwrap();
    let project_subdir = new_project_dir.path().join("subdir").join("sub-subdir");
    ensure_directory(&project_subdir, false).unwrap();
    execute_with_input(
        Command::new(scie_pants_scie)
            .arg("-V")
            .current_dir(project_subdir),
        "yes".as_bytes(),
    )
    .unwrap();
    assert!(new_project_dir.path().join("pants.toml").is_file());
}

fn test_set_pants_version(scie_pants_scie: &Path) {
    integration_test!("Verifying setting the Pants version on an existing Pants project works");
    let existing_project_dir = create_tempdir().unwrap();
    touch(&existing_project_dir.path().join("pants.toml")).unwrap();
    execute_with_input(
        Command::new(scie_pants_scie)
            .arg("-V")
            .current_dir(existing_project_dir.path()),
        "Y".as_bytes(),
    )
    .unwrap();
}

fn test_use_in_repo_with_pants_script(scie_pants_scie: &Path, clone_root: &TempDir) {
    integration_test!("Verify scie-pants can be used as `pants` in a repo with the `pants` script");
    // This verifies a fix for https://github.com/pantsbuild/scie-pants/issues/28.
    execute(
        Command::new("git")
            .args(["clone", "https://github.com/pantsbuild/example-django"])
            .current_dir(clone_root.path()),
    )
    .unwrap();
    let bin_dir = clone_root.path().join("bin");
    ensure_directory(&bin_dir, false).unwrap();
    copy(scie_pants_scie, bin_dir.join("pants").as_path()).unwrap();
    let new_path = if let Ok(existing_path) = env::var("PATH") {
        format!(
            "{bin_dir}{path_sep}{existing_path}",
            bin_dir = bin_dir.display(),
            path_sep = PATHSEP
        )
    } else {
        format!("{bin_dir}", bin_dir = bin_dir.display())
    };
    execute(
        Command::new("pants")
            .arg("-V")
            .env("PATH", new_path)
            .current_dir(clone_root.path().join("example-django")),
    )
    .unwrap();
}

fn test_dot_env_loading(scie_pants_scie: &Path, clone_root: &TempDir) {
    integration_test!(
        "Verify `.env` loading works (example-django should down grade to Pants 2.12.1)"
    );
    write_file(
        &clone_root.path().join(".env"),
        false,
        "PANTS_VERSION=2.12.1",
    )
    .unwrap();
    execute(
        Command::new(scie_pants_scie)
            .arg("-V")
            .current_dir(clone_root.path().join("example-django")),
    )
    .unwrap();
}

fn test_pants_source_mode(
    scie_pants_scie: &Path,
    clone_dir: &Path,
    pants_2_14_1_clone_dir: &Path,
    venv_dir: &Path,
    pants_2_14_1_venv_dir: &Path,
) {
    integration_test!("Verify PANTS_SOURCE mode.");
    if !pants_2_14_1_clone_dir.exists() || !pants_2_14_1_venv_dir.exists() {
        let clone_root_tmp = create_tempdir().unwrap();
        let clone_root_path = clone_root_tmp
            .path()
            .to_str()
            .with_context(|| {
                format!("Failed to convert clone root path to UTF-8 string: {clone_root_tmp:?}")
            })
            .unwrap();
        execute(Command::new("git").args(["init", clone_root_path])).unwrap();
        // N.B.: The release_2.14.1 tag has sha cfcb23a97434405a22537e584a0f4f26b4f2993b and we
        // must pass a full sha to use the shallow fetch trick.
        const PANTS_2_14_1_SHA: &str = "cfcb23a97434405a22537e584a0f4f26b4f2993b";
        execute(
            Command::new("git")
                .args([
                    "fetch",
                    "--depth",
                    "1",
                    "https://github.com/pantsbuild/pants",
                    PANTS_2_14_1_SHA,
                ])
                .current_dir(clone_root_tmp.path()),
        )
        .unwrap();
        execute(
            Command::new("git")
                .args(["reset", "--hard", PANTS_2_14_1_SHA])
                .current_dir(clone_root_tmp.path()),
        )
        .unwrap();
        write_file(
            clone_root_tmp.path().join("patch").as_path(),
            false,
            r#"
diff --git a/build-support/pants_venv b/build-support/pants_venv
index 81e3bd7..4236f4b 100755
--- a/build-support/pants_venv
+++ b/build-support/pants_venv
@@ -14,11 +14,13 @@ REQUIREMENTS=(
 # NB: We house these outside the working copy to avoid needing to gitignore them, but also to
 # dodge https://github.com/hashicorp/vagrant/issues/12057.
 platform=$(uname -mps | sed 's/ /./g')
-venv_dir_prefix="${HOME}/.cache/pants/pants_dev_deps/${platform}"
+venv_dir_prefix="${PANTS_VENV_DIR_PREFIX:-${HOME}/.cache/pants/pants_dev_deps/${platform}}"
+
+echo >&2 "The ${SCIE_PANTS_TEST_MODE:-Pants 2.14.1 clone} is working."

 function venv_dir() {
   py_venv_version=$(${PY} -c 'import sys; print("".join(map(str, sys.version_info[0:2])))')
-  echo "${venv_dir_prefix}.py${py_venv_version}.venv"
+  echo "${venv_dir_prefix}/py${py_venv_version}.venv"
 }

 function activate_venv() {
diff --git a/pants b/pants
index b422eff..16f0cf5 100755
--- a/pants
+++ b/pants
@@ -70,4 +70,5 @@ function exec_pants_bare() {
     exec ${PANTS_PREPEND_ARGS:-} "$(venv_dir)/bin/python" ${DEBUG_ARGS} "${PANTS_PY_EXE}" "$@"
 }

+echo >&2 "Pants from sources argv: $@."
 exec_pants_bare "$@"
diff --git a/pants.toml b/pants.toml
index ab5cba1..8432bb2 100644
--- a/pants.toml
+++ b/pants.toml
@@ -1,3 +1,6 @@
+[DEFAULT]
+delegate_bootstrap = true
+
 [GLOBAL]
 print_stacktrace = true

diff --git a/src/python/pants/VERSION b/src/python/pants/VERSION
index b70ae75..271706a 100644
--- a/src/python/pants/VERSION
+++ b/src/python/pants/VERSION
@@ -1 +1 @@
-2.14.1
+2.14.1+Custom-Local
\ No newline at end of file
"#,
        )
        .unwrap();
        execute(
            Command::new("git")
                .args(["apply", "patch"])
                .current_dir(clone_root_tmp.path()),
        )
        .unwrap();
        let venv_root_tmp = create_tempdir().unwrap();
        execute(
            Command::new("./pants")
                .arg("-V")
                .env("PANTS_VENV_DIR_PREFIX", venv_root_tmp.path())
                .current_dir(clone_root_tmp.path()),
        )
        .unwrap();

        remove_dir(
            clone_root_tmp
                .path()
                .join("src")
                .join("rust")
                .join("engine")
                .join("target")
                .as_path(),
        )
        .unwrap();
        ensure_directory(clone_dir, true).unwrap();
        rename(&clone_root_tmp.into_path(), pants_2_14_1_clone_dir).unwrap();
        ensure_directory(venv_dir, true).unwrap();
        rename(&venv_root_tmp.into_path(), pants_2_14_1_venv_dir).unwrap();
    }

    assert_stderr_output(
        Command::new(scie_pants_scie)
            .arg("-V")
            .env("PANTS_SOURCE", pants_2_14_1_clone_dir)
            .env("SCIE_PANTS_TEST_MODE", "PANTS_SOURCE mode")
            .env("PANTS_VENV_DIR_PREFIX", pants_2_14_1_venv_dir),
        vec![
            "The PANTS_SOURCE mode is working.",
            "Pants from sources argv: --no-verify-config -V.",
        ],
    );
}

fn test_pants_from_sources_mode(
    scie_pants_scie: &Path,
    pants_2_14_1_clone_dir: &Path,
    pants_2_14_1_venv_dir: &Path,
) {
    integration_test!("Verify pants_from_sources mode.");
    let side_by_side_root = create_tempdir().unwrap();
    let pants_dir = side_by_side_root.path().join("pants");
    softlink(pants_2_14_1_clone_dir, &pants_dir).unwrap();
    let user_repo_dir = side_by_side_root.path().join("user-repo");
    ensure_directory(&user_repo_dir, true).unwrap();
    touch(user_repo_dir.join("pants.toml").as_path()).unwrap();
    touch(user_repo_dir.join("BUILD_ROOT").as_path()).unwrap();

    let pants_from_sources = side_by_side_root.path().join("pants_from_sources");
    softlink(scie_pants_scie, &pants_from_sources).unwrap();

    assert_stderr_output(
        Command::new(pants_from_sources)
            .arg("-V")
            .env("SCIE_PANTS_TEST_MODE", "pants_from_sources mode")
            .env("PANTS_VENV_DIR_PREFIX", pants_2_14_1_venv_dir)
            .current_dir(user_repo_dir),
        vec![
            "The pants_from_sources mode is working.",
            "Pants from sources argv: --no-verify-config -V.",
        ],
    );
}

fn test_delegate_pants_in_pants_repo(scie_pants_scie: &Path, pants_2_14_1_clone_dir: &PathBuf) {
    integration_test!("Verify delegating to `./pants`.");
    assert_stderr_output(
        Command::new(scie_pants_scie)
            .arg("-V")
            .env("SCIE_PANTS_TEST_MODE", "delegate_bootstrap mode")
            .current_dir(pants_2_14_1_clone_dir),
        vec![
            "The delegate_bootstrap mode is working.",
            "Pants from sources argv: -V.",
        ],
    );
}

fn test_use_pants_release_in_pants_repo(scie_pants_scie: &Path, pants_2_14_1_clone_dir: &PathBuf) {
    let pants_release = "2.16.0.dev5";
    integration_test!("Verify usage of Pants {pants_release} on the pants repo.");
    let output = assert_stderr_output(
        Command::new(scie_pants_scie)
            .arg("help")
            .env("PANTS_VERSION", pants_release)
            .env(
                "PANTS_BACKEND_PACKAGES",
                "-[\
                    'internal_plugins.test_lockfile_fixtures',\
                    'pants.backend.explorer',\
                    ]",
            )
            .current_dir(pants_2_14_1_clone_dir)
            .stdout(Stdio::piped()),
        vec![],
    );
    let expected_message = pants_release;
    let stdout = decode_output(output.stdout).unwrap();
    assert!(
        stdout.contains(expected_message),
        "STDOUT did not contain '{expected_message}':\n{stdout}"
    );
    let unexpected_message = "Pants from sources argv";
    let stderr = decode_output(output.stderr).unwrap();
    assert!(
        !stderr.contains(unexpected_message),
        "STDERR unexpectedly contained '{unexpected_message}':\n{stderr}"
    );
}

fn test_python38_used_for_old_pants(scie_pants_scie: &Path) {
    integration_test!("Verifying Python 3.8 is selected for Pants older than 2.5.0");
    let mut command = Command::new(scie_pants_scie);
    command
        .env("PANTS_VERSION", "1.30.5rc1")
        .env(
            "PANTS_BACKEND_PACKAGES",
            "-[\
                'pants.backend.python.typecheck.mypy',\
                'pants.backend.shell',\
                'pants.backend.shell.lint.shellcheck',\
                'pants.backend.shell.lint.shfmt',\
                ]",
        )
        .args(["--no-verify-config", "--version"]);
    if Platform::MacOSX86_64 == *CURRENT_PLATFORM {
        // For unknown reasons, macOS x86_64 hangs in CI if this last test, like all prior tests
        // nonetheless!, is run with pantsd enabled mode.
        command.arg("--no-pantsd");
    }
    execute(&mut command).unwrap();
}

fn test_self_update(scie_pants_scie: &Path) {
    integration_test!("Verifying self update works");
    // N.B.: There should never be a newer release in CI; so this should always gracefully noop
    // noting no newer release was available.
    execute(Command::new(scie_pants_scie).env("SCIE_BOOT", "update")).unwrap();
}

fn test_self_downgrade(scie_pants_scie: &Path) {
    integration_test!("Verifying downgrade works");
    // Additionally, we exercise using a relative path to the scie-jump binary which triggered
    // https://github.com/pantsbuild/scie-pants/issues/38 in the past.
    execute(
        Command::new(PathBuf::from(".").join(scie_pants_scie.file_name().unwrap()))
            .env("SCIE_BOOT", "update")
            .arg("0.1.8")
            .current_dir(scie_pants_scie.parent().unwrap()),
    )
    .unwrap();
}

fn test_caching_issue_129(scie_pants_scie: &Path) {
    integration_test!(
        "Verifying the build root does not influence caching ({issue})",
        issue = issue_link(129)
    );
    let tmpdir = create_tempdir().unwrap();

    let scie_base = tmpdir.path().join("nce");

    let pants_toml = r#"
    [GLOBAL]
    pants_version = "2.15.0"
    [anonymous-telemetry]
    enabled = false
    "#;

    let one = tmpdir.path().join("one");
    ensure_directory(&one, false).unwrap();
    write_file(&one.join("pants.toml"), false, pants_toml).unwrap();
    execute(
        Command::new(scie_pants_scie)
            .arg("-V")
            .env("SCIE_BASE", &scie_base)
            .current_dir(&one),
    )
    .unwrap();

    let two = tmpdir.path().join("two");
    ensure_directory(&two, false).unwrap();
    write_file(&two.join("pants.toml"), false, pants_toml).unwrap();
    execute(
        Command::new(scie_pants_scie)
            .arg("-V")
            .env("SCIE_BASE", &scie_base)
            .current_dir(&two),
    )
    .unwrap();

    #[derive(Debug, Eq, PartialEq)]
    enum LockType {
        Configure,
        Install,
    }
    let binding_locks = walkdir::WalkDir::new(scie_base)
        .sort_by_file_name()
        .into_iter()
        .filter_map(|entry| match entry {
            Ok(dir_entry) => {
                if !dir_entry.file_type().is_file() {
                    return None;
                }
                if let Some(file_name) = dir_entry.file_name().to_str() {
                    if let Some(parent_dir) = dir_entry.path().parent() {
                        if let Some(parent_dir_name) = parent_dir.file_name() {
                            if "locks" != parent_dir_name {
                                return None;
                            }
                        }
                        if !file_name.ends_with(".lck") {
                            return None;
                        }
                        if file_name.starts_with("configure-") {
                            return Some(LockType::Configure);
                        }
                        if file_name.starts_with("install-") {
                            return Some(LockType::Install);
                        }
                    }
                }
                None
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(vec![LockType::Configure, LockType::Install], binding_locks)
}

fn test_custom_pants_toml_issue_153(scie_pants_scie: &Path) {
    integration_test!(
        "Verifying the PANTS_TOML env var is respected ({issue})",
        issue = issue_link(153)
    );

    let tmpdir = create_tempdir().unwrap();

    let buildroot = tmpdir.path().join("buildroot");
    touch(&buildroot.join("BUILD_ROOT")).unwrap();

    let pants_toml_content = r#"
    [GLOBAL]
    pants_version = "2.17.0.dev4"
    backend_packages = ["pants.backend.python"]
    [anonymous-telemetry]
    enabled = false
    "#;
    let pants_toml = tmpdir.path().join("elsewhere").join("pants.toml");
    write_file(&pants_toml, false, pants_toml_content).unwrap();

    let buildroot_subdir = buildroot.join("subdir");
    ensure_directory(&buildroot_subdir, false).unwrap();

    let output = execute(
        Command::new(scie_pants_scie)
            .arg("-V")
            .env("PANTS_TOML", &pants_toml)
            .env("PANTS_CONFIG_FILES", &pants_toml)
            .current_dir(&buildroot_subdir)
            .stdout(Stdio::piped()),
    )
    .unwrap();
    assert_eq!(
        "2.17.0.dev4",
        String::from_utf8(output.stdout.to_vec()).unwrap().trim()
    );

    let build_content = r#"
python_requirement(name="cowsay", requirements=["cowsay==5.0"])
pex_binary(name="moo", script="cowsay", dependencies=[":cowsay"])
    "#;
    write_file(&buildroot_subdir.join("BUILD"), false, build_content).unwrap();
    let output = execute(
        Command::new(scie_pants_scie)
            .args(["list", ":"])
            .env("PANTS_TOML", &pants_toml)
            .env("PANTS_CONFIG_FILES", &pants_toml)
            .current_dir(&buildroot_subdir)
            .stdout(Stdio::piped()),
    )
    .unwrap();

    let expected_output = r#"
subdir:cowsay
subdir:moo
    "#;
    assert_eq!(
        expected_output.trim(),
        String::from_utf8(output.stdout.to_vec()).unwrap().trim()
    );

    let dot_env_content = format!(
        r#"
export PANTS_TOML={pants_toml}
export PANTS_CONFIG_FILES=${{PANTS_TOML}}
        "#,
        pants_toml = pants_toml.display()
    );
    write_file(&buildroot.join(".env"), false, dot_env_content).unwrap();
    let output = execute(
        Command::new(scie_pants_scie)
            .args(["list", ":"])
            .current_dir(&buildroot_subdir)
            .stdout(Stdio::piped()),
    )
    .unwrap();
    assert_eq!(
        expected_output.trim(),
        String::from_utf8(output.stdout.to_vec()).unwrap().trim()
    );
}

fn test_pants_native_client_perms_issue_182(scie_pants_scie: &Path) {
    integration_test!(
        "Verifying scie-pants sets executable perms on the Pants native client binary when \
        present ({issue})",
        issue = issue_link(182)
    );

    let tmpdir = create_tempdir().unwrap();

    let pants_release = "2.17.0a1";
    let pants_toml_content = format!(
        r#"
        [GLOBAL]
        pants_version = "{pants_release}"
        "#
    );
    let pants_toml = tmpdir.path().join("pants.toml");
    write_file(&pants_toml, false, pants_toml_content).unwrap();

    let output = execute(
        Command::new(scie_pants_scie)
            .arg("-V")
            .current_dir(&tmpdir)
            .stdout(Stdio::piped()),
    );
    assert_eq!(
        pants_release,
        decode_output(output.unwrap().stdout).unwrap().trim()
    );
}
