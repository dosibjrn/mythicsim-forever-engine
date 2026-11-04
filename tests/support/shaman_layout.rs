//! A Shaman's talent string from the Go reference's order into the launch client's, which
//! Forever's rules read (`prepare::talent_layout`, Wowhead's talent calculator of 4 November
//! 2026): Improved Ghost Wolf moves from the sixth Enhancement square to the thirteenth, after
//! Stormstrike, and the seven talents between them shift one square earlier. Elemental and
//! Restoration keep their positions.

use serde_json::Value;

/// An old string's Enhancement tree in the launch client's order.
pub fn forever_talents(old: &str) -> String {
    let mut trees: Vec<String> = old.split('-').map(str::to_string).collect();
    if let Some(enhancement) = trees.get_mut(1) {
        let mut squares: Vec<char> = enhancement.chars().collect();
        if squares.len() > 5 {
            let ghost_wolf = squares.remove(5);
            squares.resize(squares.len().max(12), '0');
            squares.insert(12, ghost_wolf);
        }
        *enhancement = squares
            .into_iter()
            .collect::<String>()
            .trim_end_matches('0')
            .to_string();
    }
    trees.join("-")
}

/// A request whose player is a Shaman, with its talents in the launch client's order.
pub fn forever_request(request: &mut Value) {
    for party in request["raid"]["parties"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        for player in party["players"].as_array_mut().into_iter().flatten() {
            if player["class"] == "ClassShaman" {
                if let Some(old) = player["talentsString"].as_str() {
                    player["talentsString"] = Value::from(forever_talents(old));
                }
            }
        }
    }
}
