fn main() {
    let home = dancefloor::discovery::claude_home().unwrap();
    let mut app =
        dancefloor::app::App::new(home, std::time::Duration::from_secs(2), Default::default());
    app.refresh();
    for s in &app.sessions {
        println!(
            "{:?} {} -> {:?}",
            s.client,
            s.session_id,
            dancefloor::digest::write(s)
        );
    }
}
