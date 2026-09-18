//! Definition accessors shared by unit tests (flags parser etc.).

pub fn fs_read_def() -> act_kernel::CommandDef {
    act_fs::commands::read::definition().expect("definition")
}

pub fn fs_grep_def() -> act_kernel::CommandDef {
    act_fs::commands::grep::definition().expect("definition")
}

pub fn fs_edit_def() -> act_kernel::CommandDef {
    act_fs::commands::edit::definition().expect("definition")
}

pub fn fs_create_def() -> act_kernel::CommandDef {
    act_fs::commands::write::create_definition().expect("definition")
}
