use std::env;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut output_dir = PathBuf::from("certs");
    let args: Vec<String> = env::args().collect();
    let mut index = 1usize;
    while index < args.len() {
        if args[index] == "-o" {
            index += 1;
            output_dir = PathBuf::from(
                args.get(index)
                    .ok_or("missing value for -o")?,
            );
        }
        index += 1;
    }

    fs::create_dir_all(&output_dir)?;

    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    let cert_pem = cert.cert.pem();
    let key_pem = cert.signing_key.serialize_pem();

    fs::write(output_dir.join("cert.pem"), cert_pem)?;
    fs::write(output_dir.join("cert.key.pem"), key_pem)?;

    Ok(())
}
