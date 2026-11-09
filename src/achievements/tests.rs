//! The end-to-end test: login, game, frames by the virtual clock, unlock and save state, against a
//! mock server. The JSON is what RA's server returns, copied from rcheevos' own tests
//! (`test/client/test_rc_client.c`), with the console changed to the Zeebo.

use super::*;
use crate::cpu::CpuBackend;

/// Responds by the value of `r=` and keeps the requests.
struct MockServer(Rc<RefCell<Vec<Request>>>);

impl Server for MockServer {
    fn request(&mut self, request: Request, delivery: Delivery) {
        let body = request.body.clone().unwrap_or_default();
        let r = body
            .split('&')
            .find_map(|pair| pair.strip_prefix("r="))
            .unwrap_or_default();
        let response = match r {
            "login2" => {
                "{\"Success\":true,\"User\":\"User\",\"Token\":\"ApiToken\",\"Score\":12345,\
                \"SoftcoreScore\":123,\"Messages\":0,\"Permissions\":1,\"AccountType\":\"Registered\"}"
            }
            // 5501 wants byte 0x1001 equal to 1 for three frames.
            "achievementsets" => {
                "{\"Success\":true,\"GameId\":1234,\"Title\":\"Test Game\",\
                \"ConsoleId\":70,\"ImageIconUrl\":\"http://server/Images/112233.png\",\
                \"RichPresenceGameId\":1234,\"RichPresencePatch\":\"\",\"Sets\":[{\
                \"AchievementSetId\":1111,\"GameId\":1234,\"Title\":null,\"Type\":\"core\",\
                \"ImageIconUrl\":\"http://server/Images/112233.png\",\"Achievements\":[\
                {\"ID\":5501,\"Title\":\"First\",\"Description\":\"Three times\",\"Flags\":3,\
                \"Points\":5,\"MemAddr\":\"0xH1001=1.3.\",\"Author\":\"User1\",\
                \"BadgeName\":\"00234\",\"Created\":1367266583,\"Modified\":1376929305}],\
                \"Leaderboards\":[]}]}"
            }
            "startsession" => "{\"Success\":true,\"Unlocks\":[],\"HardcoreUnlocks\":[]}",
            "awardachievement" => {
                "{\"Success\":true,\"Score\":5432,\"SoftcoreScore\":128,\
                \"AchievementID\":5501,\"AchievementsRemaining\":0}"
            }
            _ => "{\"Success\":false,\"Error\":\"no mocked response\"}",
        };
        self.0.borrow_mut().push(request);
        delivery.respond(200, response.as_bytes().to_vec());
    }
}

#[test]
fn an_achievement_unlocks_end_to_end() {
    const FRAME_US: u64 = 1_000_000 / 60 + 1;
    let requests = Rc::new(RefCell::new(Vec::new()));
    let client = Rc::new(RefCell::new(Client::new(Box::new(MockServer(Rc::clone(
        &requests,
    ))))));
    assert!(client.borrow().user_agent().starts_with("Zeebx/"));
    client.borrow_mut().login_with_password("User", "Pa$$word");
    assert_eq!(client.borrow_mut().events(), vec![Event::LoggedIn(Ok(()))]);
    assert_eq!(
        client.borrow().user().map(|u| u.token),
        Some("ApiToken".into())
    );

    let mut session = crate::session::tests::minimal_session_for_save_state();
    session.achievements.hash = Some("0123456789abcdef0123456789abcdef".into());
    session.enable_achievements(Rc::clone(&client));
    let frame = |session: &mut crate::session::Session, byte: u8| {
        session
            .machine_mut()
            .cpu_mut()
            .write_mem(0x1001, &[byte])
            .unwrap();
        session.machine_mut().advance_clock_us(FRAME_US);
        session.track_achievements();
        client.borrow_mut().events()
    };

    // The first tracking only anchors. Without the clock moving, no frame is evaluated.
    session.track_achievements();
    session.track_achievements();
    assert_eq!(client.borrow_mut().events(), vec![]);
    // The game is activated on the first frame, and the achievement arms seeing the condition false.
    let game = Game {
        title: "Test Game".into(),
        icon: Some("http://server/Images/112233.png".into()),
        total: 1,
        unlocked: 0,
    };
    assert_eq!(
        frame(&mut session, 0),
        vec![Event::GameLoaded(Ok(Some(game)))]
    );
    assert_eq!(frame(&mut session, 0), vec![]);
    assert_eq!(frame(&mut session, 1), vec![]);
    assert_eq!(frame(&mut session, 1), vec![]);

    // The state carries the two hits. With them reset, going back to it makes the third enough.
    let state = session.save_state();
    client.borrow_mut().restore_progress(&[]);
    session.restore_state(&state).expect("the state loads");
    session.track_achievements(); // only anchors: the clock went where the state sent it
    let events = frame(&mut session, 1);
    assert!(
        matches!(&events[..], [Event::Unlocked(a)] if a.id == 5501 && a.points == 5),
        "{events:?}"
    );

    let award = requests
        .borrow()
        .iter()
        .filter_map(|r| r.body.clone())
        .find(|b| b.starts_with("r=awardachievement"));
    let award = award.expect("the unlock was sent");
    assert!(award.contains("a=5501") && award.contains("h=0"), "{award}");
}

/// A game extracted in the console's layout, `mod/<id>/<name>.mod` and `mif/<id>.mif`.
fn extracted_game(name: &str, with_mif: bool) -> crate::scratch::TempDir {
    let dir = crate::scratch::TempDir::new(name);
    std::fs::create_dir_all(dir.join("mod/279159")).unwrap();
    std::fs::write(dir.join("mod/279159/peteca.mod"), "BREW module: Zeebo Sports Peteca").unwrap();
    if with_mif {
        std::fs::create_dir_all(dir.join("mif")).unwrap();
        std::fs::write(dir.join("mif/279159.mif"), "MIF 279159").unwrap();
    }
    dir
}

/// The game from rcheevos' `test_hash_zeebo_extracted`: the hash of the extracted `.mod` is the
/// hash of the zip it came out of, so Zeebx can hash what is in the cache whether the package is a
/// `.zip` or a `.7z`.
#[test]
fn the_extracted_game_hash_is_the_zip_hash() {
    let dir = extracted_game("zeebx-achievements-hash", true);
    assert_eq!(
        game_hash(&dir.join("mod/279159/peteca.mod")).as_deref(),
        Some("4c1e50dc18e97935172eed419933732d")
    );
}

/// Without the `.mif` the game is not identified: the hash of the `.mod` alone wouldn't match the
/// zip's.
#[test]
fn without_the_mif_the_game_has_no_hash() {
    let dir = extracted_game("zeebx-achievements-hash-no-mif", false);
    assert_eq!(game_hash(&dir.join("mod/279159/peteca.mod")), None);
}
