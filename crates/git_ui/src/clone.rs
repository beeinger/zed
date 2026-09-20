use gpui::{App, Context, WeakEntity, Window};
use notifications::status_toast::StatusToast;
use std::path::PathBuf;
use std::sync::Arc;
use ui::{Color, Icon, IconName, IconSize, SharedString};
use util::ResultExt;
use workspace::{self, OpenMode, OpenOptions, OpenVisible, Workspace};

pub fn clone_and_open(
    repo_url: SharedString,
    workspace: WeakEntity<Workspace>,
    window: &mut Window,
    cx: &mut App,
    on_success: Arc<
        dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + Send + Sync + 'static,
    >,
) {
    let destination_prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Select as Repository Destination".into()),
    });

    window
        .spawn(cx, async move |cx| {
            let mut paths = destination_prompt.await.ok()?.ok()??;
            let mut destination_dir = paths.pop()?;

            let repo_name = repo_url
                .split('/')
                .next_back()
                .map(|name| name.strip_suffix(".git").unwrap_or(name))
                .unwrap_or("repository")
                .to_owned();

            let clone_task = workspace
                .update(cx, |workspace, cx| {
                    let fs = workspace.app_state().fs.clone();
                    let destination_dir = destination_dir.clone();
                    let repo_url = repo_url.clone();
                    cx.spawn(async move |_workspace, _cx| {
                        fs.git_clone(destination_dir.as_path(), &repo_url).await
                    })
                })
                .ok()?;

            if let Err(error) = clone_task.await {
                workspace
                    .update(cx, |workspace, cx| {
                        let toast = StatusToast::new(error.to_string(), cx, |this, _| {
                            this.icon(
                                Icon::new(IconName::XCircle)
                                    .size(IconSize::Small)
                                    .color(Color::Error),
                            )
                            .dismiss_button(true)
                        });
                        workspace.toggle_status_toast(toast, cx);
                    })
                    .log_err();
                return None;
            }

            let has_worktrees = workspace
                .read_with(cx, |workspace, cx| {
                    workspace.project().read(cx).worktrees(cx).next().is_some()
                })
                .ok()?;

            let prompt_answer = if has_worktrees {
                cx.update(|window, cx| {
                    window.prompt(
                        gpui::PromptLevel::Info,
                        &format!("Git Clone: {}", repo_name),
                        None,
                        &["Add repo to project", "Open repo in new project"],
                        cx,
                    )
                })
                .ok()?
                .await
                .ok()?
            } else {
                // Don't ask if project is empty
                0
            };

            destination_dir.push(&repo_name);

            match prompt_answer {
                0 if has_worktrees => {
                    workspace
                        .update_in(cx, |workspace, window, cx| {
                            let create_task = workspace.project().update(cx, |project, cx| {
                                project.create_worktree(destination_dir.as_path(), true, cx)
                            });

                            let workspace_weak = cx.weak_entity();
                            let on_success = on_success.clone();
                            cx.spawn_in(window, async move |_window, cx| {
                                if create_task.await.log_err().is_some() {
                                    workspace_weak
                                        .update_in(cx, |workspace, window, cx| {
                                            (on_success)(workspace, window, cx);
                                        })
                                        .ok();
                                }
                            })
                            .detach();
                        })
                        .ok()?;
                }
                0 => {
                    // FORK:local-daemon-default — empty windows share a sentinel host; open the clone as its own daemon.
                    workspace
                        .update_in(cx, |workspace, window, cx| {
                            let app_state = workspace.app_state().clone();
                            let requesting_window = window.window_handle().downcast();
                            open_cloned_destination(
                                destination_dir.clone(),
                                app_state,
                                OpenOptions {
                                    requesting_window,
                                    open_mode: OpenMode::Activate,
                                    visible: Some(OpenVisible::All),
                                    ..Default::default()
                                },
                                on_success.clone(),
                                cx,
                            );
                        })
                        .ok()?;
                }
                1 => {
                    // FORK:local-daemon-default — new-window clone is another daemon, not Project::local.
                    workspace
                        .update(cx, move |workspace, cx| {
                            let app_state = workspace.app_state().clone();
                            open_cloned_destination(
                                destination_dir.clone(),
                                app_state,
                                OpenOptions {
                                    open_mode: OpenMode::NewWindow,
                                    visible: Some(OpenVisible::All),
                                    ..Default::default()
                                },
                                on_success.clone(),
                                cx,
                            );
                        })
                        .ok();
                }
                _ => {}
            }

            Some(())
        })
        .detach();
}

fn open_cloned_destination(
    destination_path: PathBuf,
    app_state: Arc<workspace::AppState>,
    open_options: OpenOptions,
    on_success: Arc<
        dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + Send + Sync + 'static,
    >,
    cx: &mut App,
) {
    let task = workspace::open_paths(
        std::slice::from_ref(&destination_path),
        app_state,
        open_options,
        cx,
    );
    cx.spawn(async move |cx| {
        let result = task.await.log_err()?;
        result
            .window
            .update(cx, |_, window, cx| {
                result.workspace.update(cx, |workspace, cx| {
                    (on_success)(workspace, window, cx);
                })
            })
            .ok();
        Some(())
    })
    .detach();
}
