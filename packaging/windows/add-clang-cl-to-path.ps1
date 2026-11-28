# Puts LLVM's clang-cl, which aws-lc-sys builds its ARM64 code with, on the
# PATH of this step and of the job's later ones. When the runner has it
# elsewhere than on the PATH, it is taken from LLVM's own folder.
if (-not (Get-Command clang-cl -ErrorAction SilentlyContinue)) {
    $llvm = Join-Path $env:ProgramFiles 'LLVM\bin'
    $env:PATH = "$llvm;$env:PATH"
    $llvm >> $env:GITHUB_PATH
}
clang-cl --version
