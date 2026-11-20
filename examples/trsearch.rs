//! Search a localization domain for text: `trsearch <domain> <text>`
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let gd = livereplay::gamedata::GameData::new(livereplay::gamedata::find_game_dir("").unwrap());
    let path = gd.root.join(format!("res/text/lc_messages/{}.mo", a[1]));
    let b = std::fs::read(&path).unwrap();
    let rd = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let (n, ko, vo) = (rd(8), rd(12), rd(16));
    let needle = a[2].to_lowercase();
    for i in 0..n {
        let k = String::from_utf8_lossy(&b[rd(ko + 8 * i + 4)..][..rd(ko + 8 * i)]).into_owned();
        let v = String::from_utf8_lossy(&b[rd(vo + 8 * i + 4)..][..rd(vo + 8 * i)]).into_owned();
        if v.to_lowercase().contains(&needle) || k.to_lowercase().contains(&needle) {
            println!("{k} = {}", v.chars().take(80).collect::<String>());
        }
    }
}
