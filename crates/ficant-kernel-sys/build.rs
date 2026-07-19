use std::path::PathBuf;

fn main() {
    let kernel = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("cpp")
        .join("fixed-income-kernel");

    let sources = [
        "src/abi_version.cpp",
        "src/date_utils.cpp",
        "src/day_count.cpp",
        "src/bond_math.cpp",
        "src/kernel_api.cpp",
        "src/curve_math.cpp",
        "src/curve_api.cpp",
    ];

    let mut build = cc::Build::new();
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let visual_studio_llvm = PathBuf::from(
            r"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\clang-cl.exe",
        );
        let standalone_llvm = PathBuf::from(r"C:\Program Files\LLVM\bin\clang-cl.exe");
        if visual_studio_llvm.is_file() {
            build.compiler(visual_studio_llvm);
        } else if standalone_llvm.is_file() {
            build
                .compiler(standalone_llvm)
                .define("_ALLOW_COMPILER_AND_STL_VERSION_MISMATCH", None);
        }
    }
    build
        .cpp(true)
        .std("c++20")
        .flag_if_supported("/EHsc")
        .define("FICANT_KERNEL_BUILD", None)
        .include(kernel.join("include"));

    for source in sources {
        let source = kernel.join(source);
        println!("cargo:rerun-if-changed={}", source.display());
        build.file(source);
    }
    for header in [
        "include/ficant_kernel.h",
        "src/bond_math.hpp",
        "src/curve_math.hpp",
        "src/date_utils.hpp",
        "src/day_count.hpp",
    ] {
        println!("cargo:rerun-if-changed={}", kernel.join(header).display());
    }
    build.compile("ficant_kernel");
}
