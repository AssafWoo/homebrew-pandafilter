class Pandafilter < Formula
  desc "LLM token optimizer for Claude Code — 60-90% token savings on dev operations"
  homepage "https://github.com/AssafWoo/PandaFilter"
  license "MIT"
  version "1.3.19"

  depends_on "jq"

  # Prebuilt binaries — no Rust/LLVM build dependencies, installs in seconds.
  # Each tarball contains the panda binary + libonnxruntime dylib bundled together.
  on_arm do
    url "https://github.com/AssafWoo/PandaFilter/releases/download/v1.3.19/panda-macos-arm64.tar.gz"
    sha256 "973085d6545bc3e4109417ed81f01c45203b4e6809be5d9cff5dc3880de9b3c1"
  end

  on_intel do
    url "https://github.com/AssafWoo/PandaFilter/releases/download/v1.3.19/panda-macos-x86_64.tar.gz"
    sha256 "ea717ae8f5da83f04869a10fca0410997f44c8a1990e6a4a5626cd1cb410dccd"
  end

  def install
    bin.install "panda"
    # Install the bundled ORT dylib and fix rpath so the binary finds it
    dylib = Dir["libonnxruntime*.dylib"].first
    if dylib
      lib.install dylib
      system "install_name_tool", "-add_rpath", lib.to_s, "#{bin}/panda"
    end

  end

  def post_install
    # Register hooks for all detected agents (fast — no network, BERT downloads lazily on first use).
    # quiet_system suppresses output and never fails the install regardless of exit code.
    hooks_ok = quiet_system bin/"panda", "init", "--agent", "all", "--skip-model"

    if hooks_ok
      ohai "PandaFilter hooks installed for all detected agents. Run `panda doctor` to verify."
    else
      opoo "Hook setup could not complete automatically."
      puts "  Run manually after install:"
      puts "    panda init --agent all"
      puts "    panda doctor"
    end
  end

  def caveats
    <<~EOS
      Hooks are registered automatically for all detected agents on install.
      Verify your installation:
        panda doctor

      To re-run setup (e.g. after installing a new agent):
        panda init --agent all

      Then restart your coding agent for hooks to take effect.
    EOS
  end

  test do
    assert_match "filter", shell_output("#{bin}/panda --help")
    assert_match(/\S/, pipe_output("#{bin}/panda filter", "hello world\n"))
  end
end
