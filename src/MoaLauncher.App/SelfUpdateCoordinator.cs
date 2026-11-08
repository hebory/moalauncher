using System.Diagnostics;
using System.IO;
using System.Security.Cryptography;
using System.Text.Json;
using System.Text.RegularExpressions;
using MoaLauncher.Core;

namespace MoaLauncher.App;

/// <summary>Hands an already verified Inno installer to a detached copy of the launcher.</summary>
internal static class SelfUpdateCoordinator
{
    private const string HelperArgument = "--apply-self-update";
    private const string LauncherFileName = "MoaLauncher.exe";
    private static readonly TimeSpan HandshakeTimeout = TimeSpan.FromSeconds(20);
    private static readonly TimeSpan ParentExitTimeout = TimeSpan.FromSeconds(60);
    private static readonly JsonSerializerOptions JsonOptions = new() { PropertyNameCaseInsensitive = true };

    private sealed record UpdatePlan(
        string TargetDirectory,
        string DataDirectory,
        string InstallerPath,
        string Sha256,
        string Version,
        int ParentProcessId,
        long ParentStartTimeUtcTicks,
        string ParentExecutablePath,
        bool Background);

    private sealed record UpdateResult(bool Success, string Message, string LogPath, string Version, DateTimeOffset CompletedAt);

    /// <summary>
    /// Copies this app beside the downloaded installer, starts that copy as the helper,
    /// and returns only after it has validated the handoff. The caller can then shut down.
    /// </summary>
    internal static async Task PrepareHandoffAsync(
        PreparedSelfUpdate update,
        string targetDirectory,
        string dataDirectory,
        bool background,
        CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(update);
        cancellationToken.ThrowIfCancellationRequested();

        var target = NormalizeDirectory(targetDirectory, nameof(targetDirectory));
        var data = NormalizeDirectory(dataDirectory, nameof(dataDirectory));
        var installer = Path.GetFullPath(update.InstallerPath);
        var source = NormalizeDirectory(AppContext.BaseDirectory, nameof(AppContext.BaseDirectory));
        var processPath = Environment.ProcessPath;
        if (string.IsNullOrWhiteSpace(processPath) || !Path.GetFileName(processPath).Equals(LauncherFileName, StringComparison.OrdinalIgnoreCase))
            throw new InvalidOperationException("런처를 게시된 MoaLauncher.exe로 실행한 경우에만 자체 업데이트할 수 있습니다.");
        if (!SamePath(Path.GetDirectoryName(Path.GetFullPath(processPath))!, target) || !SamePath(source, target))
            throw new InvalidOperationException("실행 중인 런처와 업데이트 대상 폴더가 다릅니다.");

        ValidateTarget(target);
        ValidateDataAndInstaller(data, installer);
        EnsureDisjoint(target, data, "런처 설치 폴더와 데이터 폴더가 겹칩니다.");
        RejectReparsePath(source);
        RejectReparsePath(target);
        RejectReparsePath(data);

        var expectedHash = NormalizeHash(update.Sha256);
        var actualHash = await ComputeSha256Async(installer, cancellationToken).ConfigureAwait(false);
        if (!CryptographicOperations.FixedTimeEquals(Convert.FromHexString(expectedHash), Convert.FromHexString(actualHash)))
            throw new InvalidDataException("자체 업데이트 설치 파일의 SHA-256 검증에 실패했습니다.");
        if (!IsSafeVersion(update.Version))
            throw new InvalidDataException("자체 업데이트 버전 형식이 올바르지 않습니다.");

        ProbeWritable(target);
        ProbeWritable(Path.GetDirectoryName(installer)!);

        var jobRoot = Path.GetDirectoryName(installer)!;
        var runner = Path.Combine(jobRoot, "runner");
        if (IsSameOrWithin(target, runner) || IsSameOrWithin(runner, target))
            throw new InvalidOperationException("런처와 자체 업데이트 helper 폴더가 겹칩니다.");
        if (Directory.Exists(runner) || File.Exists(runner))
            throw new IOException("자체 업데이트 helper 폴더가 이미 있습니다.");

        var parent = Process.GetCurrentProcess();
        var plan = new UpdatePlan(target, data, installer, expectedHash, NormalizeVersion(update.Version), parent.Id,
            parent.StartTime.ToUniversalTime().Ticks, Path.GetFullPath(processPath), background);
        var planPath = Path.Combine(jobRoot, "plan.json");
        var readyPath = Path.Combine(jobRoot, "helper.ready");
        if (File.Exists(planPath) || File.Exists(readyPath))
            throw new IOException("자체 업데이트 handoff 파일이 이미 있습니다.");

        Process? helper = null;
        try
        {
            CopyApplicationDirectory(source, runner);
            await WriteJsonAtomicallyAsync(planPath, plan, cancellationToken).ConfigureAwait(false);
            var helperExe = Path.Combine(runner, LauncherFileName);
            if (!File.Exists(helperExe)) throw new FileNotFoundException("자체 업데이트 helper 실행 파일이 없습니다.", helperExe);

            var start = new ProcessStartInfo(helperExe)
            {
                UseShellExecute = false,
                CreateNoWindow = true,
                WindowStyle = ProcessWindowStyle.Hidden,
                WorkingDirectory = runner
            };
            start.ArgumentList.Add(HelperArgument);
            start.ArgumentList.Add(planPath);
            helper = Process.Start(start) ?? throw new InvalidOperationException("자체 업데이트 helper를 시작하지 못했습니다.");

            var deadline = DateTime.UtcNow + HandshakeTimeout;
            while (DateTime.UtcNow < deadline)
            {
                cancellationToken.ThrowIfCancellationRequested();
                if (File.Exists(readyPath)) return;
                if (helper.HasExited)
                    throw new InvalidOperationException($"자체 업데이트 helper가 준비되지 않았습니다. 종료 코드: {helper.ExitCode}");
                await Task.Delay(100, cancellationToken).ConfigureAwait(false);
            }
            throw new TimeoutException("자체 업데이트 helper의 준비 응답 시간이 초과되었습니다.");
        }
        catch
        {
            if (helper is { HasExited: false })
            {
                try { helper.Kill(entireProcessTree: false); helper.WaitForExit(10_000); } catch { }
            }
            // Do not remove files while a helper may still be waiting on this parent.
            if (helper is null || helper.HasExited)
            {
                TryDeleteFile(planPath);
                TryDeleteFile(planPath + ".tmp");
                TryDeleteFile(readyPath);
                TryDeleteDirectory(runner);
            }
            throw;
        }
        finally { helper?.Dispose(); }
    }

    /// <summary>Entry point called by App.OnStartup when --apply-self-update is present.</summary>
    internal static Task<int> RunHelperAsync(string planPath) => Task.Run(() => RunHelper(planPath));

    private static int RunHelper(string planPath)
    {
        string? validatedData = null;
        string? validatedTarget = null;
        UpdatePlan? plan = null;
        string logPath = string.Empty;
        var parentExited = false;
        var installerStarted = false;
        var installationVerified = false;
        var restartAttempted = false;
        string? runnerDirectory = null;
        try
        {
            planPath = Path.GetFullPath(planPath);
            RejectReparsePath(planPath);
            plan = JsonSerializer.Deserialize<UpdatePlan>(File.ReadAllText(planPath), JsonOptions)
                ?? throw new InvalidDataException("자체 업데이트 계획을 읽을 수 없습니다.");
            validatedData = NormalizeDirectory(plan.DataDirectory, nameof(plan.DataDirectory));
            var jobRoot = Path.GetDirectoryName(planPath)!;
            var expectedJobRoot = Path.Combine(validatedData, "self-update", Path.GetFileName(jobRoot));
            if (!SamePath(jobRoot, expectedJobRoot) || !Guid.TryParse(Path.GetFileName(jobRoot), out _))
                throw new InvalidDataException("자체 업데이트 작업 폴더가 올바르지 않습니다.");
            RejectReparsePath(validatedData);
            RejectReparsePath(jobRoot);

            var runner = NormalizeDirectory(AppContext.BaseDirectory, nameof(AppContext.BaseDirectory));
            runnerDirectory = runner;
            RejectReparsePath(runner);
            if (!SamePath(Path.GetDirectoryName(runner)!, jobRoot) || !Path.GetFileName(runner).Equals("runner", StringComparison.OrdinalIgnoreCase))
                throw new InvalidDataException("helper 실행 위치가 자체 업데이트 작업 폴더와 일치하지 않습니다.");

            var target = NormalizeDirectory(plan.TargetDirectory, nameof(plan.TargetDirectory));
            validatedTarget = target;
            var expectedParentExecutable = Path.Combine(target, LauncherFileName);
            if (!SamePath(plan.ParentExecutablePath, expectedParentExecutable))
                throw new InvalidDataException("handoff를 요청한 런처 경로가 설치 대상 실행 파일과 다릅니다.");
            var installer = Path.GetFullPath(plan.InstallerPath);
            if (!SamePath(Path.GetDirectoryName(installer)!, jobRoot) || !IsInstallerFileName(Path.GetFileName(installer)))
                throw new InvalidDataException("설치 파일 경로가 작업 폴더를 벗어났습니다.");
            ValidateTarget(target);
            EnsureDisjoint(target, validatedData, "런처 설치 폴더와 데이터 폴더가 겹칩니다.");
            RejectReparsePath(target);
            RejectReparsePath(installer);
            if (!IsSafeVersion(plan.Version) || !Regex.IsMatch(plan.Sha256, "^[0-9a-fA-F]{64}$", RegexOptions.CultureInvariant))
                throw new InvalidDataException("업데이트 버전 또는 체크섬 형식이 올바르지 않습니다.");
            var hash = ComputeSha256(installer);
            if (!CryptographicOperations.FixedTimeEquals(Convert.FromHexString(plan.Sha256), Convert.FromHexString(hash)))
                throw new InvalidDataException("설치 파일의 SHA-256 검증에 실패했습니다.");
            ValidateParent(plan);

            var readyPath = Path.Combine(jobRoot, "helper.ready");
            using (var ready = new FileStream(readyPath, FileMode.CreateNew, FileAccess.Write, FileShare.Read))
            using (var writer = new StreamWriter(ready))
            {
                writer.Write(Environment.ProcessId);
            }

            if (!WaitForParentExit(plan))
            {
                WriteResult(validatedData, new UpdateResult(false, "런처가 종료되지 않아 업데이트를 시작하지 않았습니다.", string.Empty, plan.Version, DateTimeOffset.Now));
                return 2;
            }
            parentExited = true;

            var identity = Convert.ToHexString(SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(validatedData.ToUpperInvariant())))[..16];
            using var mutex = new Mutex(false, @"Local\MoaLauncher-" + identity);
            var ownsMutex = false;
            Exception? installFailure = null;
            try
            {
                try { ownsMutex = mutex.WaitOne(TimeSpan.FromSeconds(10)); }
                catch (AbandonedMutexException) { ownsMutex = true; }
                if (!ownsMutex) throw new InvalidOperationException("다른 런처 인스턴스가 실행 중이어서 업데이트를 시작하지 않았습니다.");

                logPath = Path.Combine(jobRoot, "install.log");
                var start = new ProcessStartInfo(installer)
                {
                    UseShellExecute = false,
                    CreateNoWindow = true,
                    WindowStyle = ProcessWindowStyle.Hidden,
                    WorkingDirectory = jobRoot
                };
                foreach (var arg in new[] { "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-", "/NOCLOSEAPPLICATIONS", "/NORESTARTAPPLICATIONS", "/DIR=" + target, "/LOG=" + logPath })
                    start.ArgumentList.Add(arg);
                using var installerProcess = Process.Start(start) ?? throw new InvalidOperationException("업데이트 설치 프로그램을 시작하지 못했습니다.");
                installerStarted = true;
                installerProcess.WaitForExit();
                if (installerProcess.ExitCode != 0)
                    throw new InvalidOperationException($"설치 프로그램이 오류 코드 {installerProcess.ExitCode}로 종료되었습니다.");

                var installedExe = Path.Combine(target, LauncherFileName);
                var actualVersion = ReadProductVersion(installedExe);
                if (actualVersion is null || !VersionsMatch(actualVersion, plan.Version))
                    throw new InvalidDataException("설치 후 런처 버전 확인에 실패했습니다.");
                installationVerified = true;
            }
            catch (Exception ex)
            {
                installFailure = ex;
            }
            finally
            {
                if (ownsMutex) mutex.ReleaseMutex();
            }

            if (installFailure is not null)
            {
                if (installerStarted)
                {
                    try
                    {
                        RestorePreviousApp(runner, target);
                    }
                    catch (Exception restoreError)
                    {
                        var message = "업데이트 실패 후 이전 실행 파일 복구에도 실패했습니다: " + restoreError.Message + " 백업 위치: " + runner;
                        WriteResult(validatedData, new UpdateResult(false, message, logPath, plan.Version, DateTimeOffset.Now));
                        return 1;
                    }

                    var restoredMessage = "업데이트 실패. 이전 실행 파일로 복구했습니다. " + installFailure.Message + " 백업 위치: " + runner;
                    WriteResult(validatedData, new UpdateResult(false, restoredMessage, logPath, plan.Version, DateTimeOffset.Now));
                    var restartOkAfterRestore = TryRestartLauncher(plan, target, out var restartErrorAfterRestore);
                    restartAttempted = true;
                    if (!restartOkAfterRestore)
                        WriteResult(validatedData, new UpdateResult(false, restoredMessage + " 런처 재시작 실패: " + restartErrorAfterRestore, logPath, plan.Version, DateTimeOffset.Now));
                    return 1;
                }

                var failureMessage = "Moa Launcher 업데이트에 실패했습니다: " + installFailure.Message;
                WriteResult(validatedData, new UpdateResult(false, failureMessage, logPath, plan.Version, DateTimeOffset.Now));
                var restartedAfterFailure = TryRestartLauncher(plan, target, out var restartError);
                restartAttempted = true;
                if (!restartedAfterFailure)
                    WriteResult(validatedData, new UpdateResult(false, failureMessage + " 런처 재시작 실패: " + restartError, logPath, plan.Version, DateTimeOffset.Now));
                return 1;
            }

            const string successMessage = "Moa Launcher 업데이트를 완료했습니다.";
            WriteResult(validatedData, new UpdateResult(true, successMessage, logPath, plan.Version, DateTimeOffset.Now));
            var restartOk = TryRestartLauncher(plan, target, out var successRestartError);
            restartAttempted = true;
            if (!restartOk)
                WriteResult(validatedData, new UpdateResult(false, "Moa Launcher 업데이트는 완료했지만 재시작하지 못했습니다: " + successRestartError, logPath, plan.Version, DateTimeOffset.Now));
            return restartOk ? 0 : 3;
        }
        catch (Exception ex)
        {
            if (validatedData is not null && validatedTarget is not null && plan is not null && parentExited)
            {
                var message = "Moa Launcher 업데이트 helper 오류: " + ex.Message;
                var canRestart = true;
                if (installerStarted && !installationVerified)
                {
                    try
                    {
                        if (runnerDirectory is null) throw new InvalidOperationException("실행 파일 백업 폴더가 없습니다.");
                        RestorePreviousApp(runnerDirectory, validatedTarget);
                        message += " 이전 실행 파일로 복구했습니다. 백업 위치: " + runnerDirectory;
                    }
                    catch (Exception restoreError)
                    {
                        message += " 이전 실행 파일 복구에도 실패했습니다: " + restoreError.Message + " 백업 위치: " + (runnerDirectory ?? "알 수 없음");
                        canRestart = false;
                    }
                }

                try { WriteResult(validatedData, new UpdateResult(false, message, logPath, plan.Version, DateTimeOffset.Now)); }
                catch { return 1; }
                if (canRestart && !restartAttempted)
                {
                    var restarted = TryRestartLauncher(plan, validatedTarget, out var restartError);
                    restartAttempted = true;
                    if (!restarted)
                    {
                        message += " 런처 재시작 실패: " + restartError;
                        try { WriteResult(validatedData, new UpdateResult(false, message, logPath, plan.Version, DateTimeOffset.Now)); }
                        catch { }
                    }
                }
            }
            return 1;
        }
    }

    private static bool WaitForParentExit(UpdatePlan plan)
    {
        var deadline = DateTime.UtcNow + ParentExitTimeout;
        while (DateTime.UtcNow < deadline)
        {
            try
            {
                using var parent = Process.GetProcessById(plan.ParentProcessId);
                if (parent.HasExited) return true;
                try
                {
                    if (Math.Abs(parent.StartTime.ToUniversalTime().Ticks - plan.ParentStartTimeUtcTicks) > TimeSpan.FromSeconds(2).Ticks)
                        return true; // The original process ended and its PID was reused.
                    var path = parent.MainModule?.FileName;
                    if (path is not null && !SamePath(path, plan.ParentExecutablePath)) return true;
                }
                catch (Exception ex) when (ex is InvalidOperationException or System.ComponentModel.Win32Exception or UnauthorizedAccessException)
                {
                    // An inaccessible process is not evidence that the original one exited.
                }
            }
            catch (ArgumentException) { return true; }
            catch (InvalidOperationException) { return true; }
            Thread.Sleep(150);
        }
        return false;
    }

    private static void ValidateParent(UpdatePlan plan)
    {
        try
        {
            using var parent = Process.GetProcessById(plan.ParentProcessId);
            if (parent.HasExited || !MatchesParent(parent, plan))
                throw new InvalidDataException("업데이트 handoff를 요청한 런처 프로세스가 일치하지 않습니다.");
        }
        catch (ArgumentException ex) { throw new InvalidDataException("업데이트 handoff 런처가 실행 중이 아닙니다.", ex); }
    }

    private static bool MatchesParent(Process process, UpdatePlan plan)
    {
        try
        {
            var startTicks = process.StartTime.ToUniversalTime().Ticks;
            var tolerance = TimeSpan.FromSeconds(2).Ticks;
            var sameStart = Math.Abs(startTicks - plan.ParentStartTimeUtcTicks) <= tolerance;
            var path = process.MainModule?.FileName;
            return sameStart && path is not null && SamePath(path, plan.ParentExecutablePath);
        }
        catch (Exception ex) when (ex is InvalidOperationException or System.ComponentModel.Win32Exception or UnauthorizedAccessException)
        {
            return false;
        }
    }

    private static void ValidateTarget(string target)
    {
        var exe = Path.Combine(target, LauncherFileName);
        var dll = Path.Combine(target, "MoaLauncher.dll");
        if (!File.Exists(exe) || !File.Exists(dll))
            throw new InvalidOperationException("업데이트 대상은 MoaLauncher.exe와 MoaLauncher.dll이 있는 런처 폴더여야 합니다.");
        RejectReparsePath(exe);
        RejectReparsePath(dll);
    }

    private static void ValidateDataAndInstaller(string data, string installer)
    {
        if (!Directory.Exists(data)) throw new DirectoryNotFoundException("런처 데이터 폴더를 찾을 수 없습니다.");
        var root = Path.Combine(data, "self-update");
        var job = Path.GetDirectoryName(installer)!;
        if (!SamePath(Path.GetDirectoryName(job)!, root) || !Guid.TryParse(Path.GetFileName(job), out _) || !IsInstallerFileName(Path.GetFileName(installer)))
            throw new InvalidDataException("자체 업데이트 설치 파일이 예상한 데이터 폴더 안에 없습니다.");
        if (!File.Exists(installer)) throw new FileNotFoundException("자체 업데이트 설치 파일을 찾을 수 없습니다.", installer);
        RejectReparsePath(root);
        RejectReparsePath(job);
        RejectReparsePath(installer);
    }

    private static void CopyApplicationDirectory(string source, string destination, bool overwrite = false)
    {
        source = NormalizeDirectory(source, nameof(source));
        destination = NormalizeDirectory(destination, nameof(destination));
        EnsureDisjoint(source, destination, "launcher-files 복사 원본과 대상 폴더가 겹칩니다.");
        RejectReparsePath(source);
        RejectReparsePath(destination);

        var manifestPath = SafePaths.ResolveRelative(source, "launcher-files.json");
        RejectReparsePath(manifestPath);
        if (!File.Exists(manifestPath))
            throw new InvalidDataException("launcher-files.json이 없습니다. 게시된 런처로 자체 업데이트를 실행해 주세요.");
        var manifestInfo = new FileInfo(manifestPath);
        if (manifestInfo.Length <= 0 || manifestInfo.Length > 1024 * 1024)
            throw new InvalidDataException("launcher-files.json 크기가 허용 범위를 벗어났습니다.");

        string[] files;
        try { files = JsonSerializer.Deserialize<string[]>(File.ReadAllText(manifestPath), JsonOptions) ?? throw new InvalidDataException("launcher-files.json 형식이 올바르지 않습니다."); }
        catch (JsonException ex) { throw new InvalidDataException("launcher-files.json 형식이 올바르지 않습니다.", ex); }
        if (files.Length is 0 or > 4096)
            throw new InvalidDataException("launcher-files.json 파일 항목 수가 허용 범위를 벗어났습니다.");

        var unique = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var required = new[] { "launcher-files.json", LauncherFileName, "MoaLauncher.dll", "MoaLauncher.deps.json", "MoaLauncher.runtimeconfig.json", "MoaLauncher.Core.dll" };
        foreach (var requiredPath in required)
            if (!files.Contains(requiredPath, StringComparer.OrdinalIgnoreCase))
                throw new InvalidDataException($"launcher-files.json에 필수 파일 {requiredPath}이 없습니다.");

        var resolved = new List<(string Relative, string Source, string Destination)>();
        long totalBytes = 0;
        foreach (var relative in files)
        {
            if (string.IsNullOrWhiteSpace(relative) || relative.Length > 1024 || !unique.Add(relative))
                throw new InvalidDataException("launcher-files.json에 빈 경로, 긴 경로 또는 중복 경로가 있습니다.");
            var sourceFile = SafePaths.ResolveRelative(source, relative);
            var destinationFile = SafePaths.ResolveRelative(destination, relative);
            RejectReparsePath(sourceFile);
            RejectReparsePath(destinationFile);
            if (!File.Exists(sourceFile))
                throw new FileNotFoundException("launcher-files.json에 지정된 파일이 없습니다.", sourceFile);
            var length = new FileInfo(sourceFile).Length;
            totalBytes = checked(totalBytes + length);
            if (totalBytes > 1024L * 1024 * 1024)
                throw new InvalidDataException("launcher-files.json 파일 전체 크기가 1GB를 넘습니다.");
            resolved.Add((relative, sourceFile, destinationFile));
        }

        Directory.CreateDirectory(destination);
        foreach (var file in resolved)
        {
            var destinationDirectory = Path.GetDirectoryName(file.Destination)!;
            Directory.CreateDirectory(destinationDirectory);
            RejectReparsePath(destinationDirectory);
            if (!overwrite && File.Exists(file.Destination))
                throw new IOException("launcher-files 복사 대상 파일이 이미 있습니다: " + file.Relative);

            var mode = overwrite ? FileMode.Create : FileMode.CreateNew;
            using var input = new FileStream(file.Source, FileMode.Open, FileAccess.Read, FileShare.Read);
            using var output = new FileStream(file.Destination, mode, FileAccess.Write, FileShare.None);
            input.CopyTo(output);
            output.Flush(true);
        }

        foreach (var requiredPath in required.Skip(1))
        {
            var destinationFile = SafePaths.ResolveRelative(destination, requiredPath);
            if (!File.Exists(destinationFile))
                throw new IOException("launcher-files 복사 후 필수 실행 파일이 없습니다: " + requiredPath);
        }
    }

    private static async Task WriteJsonAtomicallyAsync<T>(string path, T value, CancellationToken cancellationToken)
    {
        var temp = path + ".tmp";
        await using (var stream = new FileStream(temp, FileMode.CreateNew, FileAccess.Write, FileShare.None, 4096, useAsync: true))
        {
            await JsonSerializer.SerializeAsync(stream, value, JsonOptions, cancellationToken).ConfigureAwait(false);
            await stream.FlushAsync(cancellationToken).ConfigureAwait(false);
            stream.Flush(true);
        }
        File.Move(temp, path);
    }

    private static void WriteResult(string dataDirectory, UpdateResult result)
    {
        var path = Path.Combine(dataDirectory, "self-update-result.json");
        RejectReparsePath(path);
        var temp = path + ".tmp-" + Guid.NewGuid().ToString("N");
        using (var stream = new FileStream(temp, FileMode.CreateNew, FileAccess.Write, FileShare.None))
        {
            JsonSerializer.Serialize(stream, result, JsonOptions);
            stream.Flush(true);
        }
        File.Move(temp, path, true);
    }

    private static void RestorePreviousApp(string backupDirectory, string targetDirectory)
    {
        var backupExecutable = Path.Combine(backupDirectory, LauncherFileName);
        var backupVersion = ReadProductVersion(backupExecutable)
            ?? throw new InvalidDataException("백업 런처의 버전을 확인할 수 없습니다.");
        CopyApplicationDirectory(backupDirectory, targetDirectory, overwrite: true);
        ValidateTarget(targetDirectory);
        var restoredVersion = ReadProductVersion(Path.Combine(targetDirectory, LauncherFileName));
        if (restoredVersion is null || !VersionsMatch(restoredVersion, backupVersion))
            throw new InvalidDataException("복구 후 이전 런처 버전을 확인할 수 없습니다.");
    }

    private static bool TryRestartLauncher(UpdatePlan plan, string target, out string error)
    {
        try
        {
            var exe = Path.Combine(target, LauncherFileName);
            if (!File.Exists(exe))
            {
                error = "실행 파일을 찾을 수 없습니다: " + exe;
                return false;
            }
            var start = new ProcessStartInfo(exe)
            {
                UseShellExecute = false,
                WorkingDirectory = target,
                CreateNoWindow = plan.Background,
                WindowStyle = plan.Background ? ProcessWindowStyle.Hidden : ProcessWindowStyle.Normal
            };
            start.ArgumentList.Add("--data-dir");
            start.ArgumentList.Add(plan.DataDirectory);
            if (plan.Background) start.ArgumentList.Add(WindowsStartupRegistration.BackgroundArgument);
            using var process = Process.Start(start);
            if (process is null)
            {
                error = "런처 프로세스를 시작하지 못했습니다.";
                return false;
            }
            error = string.Empty;
            return true;
        }
        catch (Exception ex)
        {
            error = ex.Message;
            return false;
        }
    }

    private static string? ReadProductVersion(string executable)
    {
        if (!File.Exists(executable)) return null;
        try { return FileVersionInfo.GetVersionInfo(executable).ProductVersion?.Split('+')[0]; }
        catch { return null; }
    }

    private static bool VersionsMatch(string actual, string expected) =>
        string.Equals(NormalizeVersion(actual), NormalizeVersion(expected), StringComparison.OrdinalIgnoreCase);

    private static bool IsSafeVersion(string version) => Regex.IsMatch(version, "^v?[0-9]+(?:\\.[0-9]+){1,3}(?:[-+][A-Za-z0-9.-]+)?$", RegexOptions.CultureInvariant);

    private static bool IsInstallerFileName(string name) =>
        Regex.IsMatch(name, "^MoaLauncher-[A-Za-z0-9.+-]+-Setup-x64\\.exe$", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);

    private static string NormalizeVersion(string version)
    {
        var value = version.Trim().TrimStart('v', 'V').Split('+')[0];
        var parts = value.Split('-')[0].Split('.').ToList();
        // Inno's Windows ProductVersion commonly includes a fourth zero component.
        if (parts.Count == 4 && parts[3] == "0") parts.RemoveAt(3);
        return string.Join('.', parts);
    }

    private static string NormalizeHash(string value)
    {
        if (value is null || !Regex.IsMatch(value, "^[0-9a-fA-F]{64}$", RegexOptions.CultureInvariant))
            throw new InvalidDataException("업데이트 SHA-256 값이 올바르지 않습니다.");
        return value.ToLowerInvariant();
    }

    private static async Task<string> ComputeSha256Async(string path, CancellationToken cancellationToken)
    {
        await using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read, 128 * 1024, useAsync: true);
        return Convert.ToHexString(await SHA256.HashDataAsync(stream, cancellationToken).ConfigureAwait(false)).ToLowerInvariant();
    }

    private static string ComputeSha256(string path)
    {
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        return Convert.ToHexString(SHA256.HashData(stream)).ToLowerInvariant();
    }

    private static string NormalizeDirectory(string path, string parameterName)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(path, parameterName);
        if (!Path.IsPathFullyQualified(path) || path.IndexOfAny(new[] { '\0', '"', '\r', '\n' }) >= 0)
            throw new ArgumentException("전체 경로가 필요합니다.", parameterName);
        var fullPath = Path.GetFullPath(path);
        if (SamePath(fullPath, Path.GetPathRoot(fullPath)!))
            throw new ArgumentException("드라이브 또는 파일 시스템 루트는 사용할 수 없습니다.", parameterName);
        return fullPath.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
    }

    private static void EnsureDisjoint(string first, string second, string message)
    {
        if (IsSameOrWithin(first, second) || IsSameOrWithin(second, first)) throw new InvalidOperationException(message);
    }

    private static bool IsSameOrWithin(string root, string path) => SamePath(root, path) ||
        path.StartsWith(root.TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase);

    private static bool SamePath(string first, string second) =>
        string.Equals(Path.GetFullPath(first).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar),
            Path.GetFullPath(second).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar), StringComparison.OrdinalIgnoreCase);

    private static void RejectReparsePath(string path)
    {
        var full = Path.GetFullPath(path);
        var root = Path.GetPathRoot(full) ?? throw new InvalidDataException("경로 루트를 확인할 수 없습니다.");
        var current = root;
        foreach (var part in full[root.Length..].Split(new[] { Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar }, StringSplitOptions.RemoveEmptyEntries))
        {
            current = Path.Combine(current, part);
            if (!File.Exists(current) && !Directory.Exists(current)) continue;
            var attributes = File.GetAttributes(current);
            if ((attributes & FileAttributes.ReparsePoint) != 0)
                throw new InvalidDataException("자체 업데이트 경로에 링크 또는 재분석 지점이 포함되어 있습니다.");
        }
    }

    private static void ProbeWritable(string directory)
    {
        Directory.CreateDirectory(directory);
        var path = Path.Combine(directory, ".moa-self-update-check-" + Guid.NewGuid().ToString("N"));
        using (new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None, 1, FileOptions.DeleteOnClose)) { }
    }

    private static void TryDeleteFile(string path)
    {
        try { if (File.Exists(path)) File.Delete(path); } catch { }
    }

    private static void TryDeleteDirectory(string path)
    {
        try { if (Directory.Exists(path)) Directory.Delete(path, recursive: true); } catch { }
    }
}
