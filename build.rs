fn main() {
    tonic_build::configure()
        .compile(&["proto/v1beta1.proto"], &["proto/"])
        .unwrap();
}
