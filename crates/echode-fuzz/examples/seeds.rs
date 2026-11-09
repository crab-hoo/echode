//! Writes a target's seeds into a directory, one file each, for `make fuzz`.
//!
//! Usage: `cargo run -p echode-fuzz --example seeds -- TARGET DIRECTORY`.

use echode_fuzz::Target;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(name), Some(directory), None) = (args.next(), args.next(), args.next()) else {
        eprintln!("usage: seeds TARGET DIRECTORY");
        std::process::exit(2);
    };
    let Some(target) = Target::from_name(&name) else {
        eprintln!("unknown target {name}");
        std::process::exit(2);
    };
    let directory = std::path::Path::new(&directory);
    std::fs::create_dir_all(directory).expect("create the corpus directory");
    for (index, seed) in target.seeds().iter().enumerate() {
        std::fs::write(directory.join(format!("seed-{index:03}")), seed).expect("write a seed");
    }
    echode_fuzz::remove_scratch();
}
