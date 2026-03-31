class Ccr < Formula
  desc "LLM token optimizer for Claude Code — 60-90% token savings on dev operations"
  homepage "https://github.com/AssafWoo/homebrew-ccr"
  license "MIT"
  version "0.5.25"

  # Prebuilt binaries — no Rust/LLVM build dependencies, installs in seconds.
  # Each tarball contains the ccr binary + libonnxruntime dylib bundled together.
  on_arm do
    url "https://github.com/AssafWoo/homebrew-ccr/releases/download/v0.5.25/ccr-macos-arm64.tar.gz"
    sha256 "a4475788b9e347aeee08bc4a878d48e6d73648e3befa558f3beb4d82627b15ca"
  end

  on_intel do
    url "https://github.com/AssafWoo/homebrew-ccr/releases/download/v0.5.25/ccr-macos-x86_64.tar.gz"
    sha256 "1478b162dc6eaa1242c7b9f2aae7bc71bedeee2c7c69ba4dd9da4224855f4f13"
  end

  def install
    bin.install "ccr"
    # Install the bundled ORT dylib and fix rpath so the binary finds it
    dylib = Dir["libonnxruntime*.dylib"].first
    if dylib
      lib.install dylib
      system "install_name_tool", "-add_rpath", lib.to_s, "#{bin}/ccr"
    end
  end

  def post_install
    # Pre-download the BERT model and register Claude Code hooks automatically.
    # Runs as the installing user so ~/.cache and ~/.claude are correct.
    # quiet_system — don't fail the install if Claude Code isn't set up yet.
    quiet_system bin/"ccr", "init"
  end

  def caveats
    <<~EOS
      CCR setup runs automatically during install (hooks + BERT model download).
      If you see Claude Code hook errors, run manually:
        ccr init
    EOS
  end

  test do
    assert_match "filter", shell_output("#{bin}/ccr --help")
    assert_match(/\S/, pipe_output("#{bin}/ccr filter", "hello world\n"))
  end
end
