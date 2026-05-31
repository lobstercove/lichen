use lichen_core::{Keypair, KeypairFile};
use std::env;
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!(
        "Usage: cargo run -p lichen-rpc --bin keypair_from_seed_byte -- \
  --seed-byte N [--output PATH]"
    );
    std::process::exit(2);
}

fn next_arg(args: &[String], index: &mut usize, flag: &str) -> String {
    *index += 1;
    if *index >= args.len() {
        eprintln!("missing value for {}", flag);
        usage();
    }
    args[*index].clone()
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        usage();
    }

    let mut seed_byte = None;
    let mut output: Option<PathBuf> = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--seed-byte" => {
                seed_byte = Some(
                    next_arg(&args, &mut index, "--seed-byte")
                        .parse::<u8>()
                        .unwrap_or_else(|_| {
                            eprintln!("--seed-byte must fit in u8");
                            usage();
                        }),
                );
            }
            "--output" => output = Some(PathBuf::from(next_arg(&args, &mut index, "--output"))),
            unknown => {
                eprintln!("unknown argument: {}", unknown);
                usage();
            }
        }
        index += 1;
    }

    let seed_byte = seed_byte.unwrap_or_else(|| usage());
    let keypair = Keypair::from_seed(&[seed_byte; 32]);
    let keypair_file = KeypairFile::from_keypair(&keypair);

    if let Some(path) = output {
        keypair_file
            .save(&path)
            .unwrap_or_else(|error| panic!("write keypair {}: {}", path.display(), error));
        println!("{}", keypair.pubkey().to_base58());
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&keypair_file).expect("encode keypair")
        );
    }
}
