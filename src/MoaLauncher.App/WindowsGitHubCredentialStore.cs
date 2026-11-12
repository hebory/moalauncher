using System.IO;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using MoaLauncher.Core;

namespace MoaLauncher.App;

internal interface IGitHubCredentialStore : IGitHubTokenProvider
{
    void SaveToken(string token);
    void DeleteToken();
}

/// <summary>Keeps a GitHub personal access token in the current Windows user's Credential Manager.</summary>
internal sealed class WindowsGitHubCredentialStore : IGitHubCredentialStore
{
    private const uint GenericCredential = 1;
    private const uint PersistLocalMachine = 2;
    private const int ErrorNotFound = 1168;
    private const int MaximumCredentialBlobBytes = 2560;
    private const string CredentialTargetPrefix = "MoaLauncher/GitHub/";
    private static readonly UTF8Encoding StrictUtf8 = new(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true);
    private readonly string targetName;

    public WindowsGitHubCredentialStore(string dataDirectory)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(dataDirectory);
        var normalized = Path.TrimEndingDirectorySeparator(Path.GetFullPath(dataDirectory)).ToUpperInvariant();
        var hash = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(normalized))).ToLowerInvariant();
        targetName = CredentialTargetPrefix + hash;
    }

    public string? GetToken()
    {
        if (!CredReadW(targetName, GenericCredential, 0, out var credentialPointer))
        {
            var error = Marshal.GetLastWin32Error();
            if (error == ErrorNotFound) return null;
            throw new InvalidOperationException("Windows 자격 증명에서 GitHub 인증 정보를 읽지 못했습니다.");
        }

        try
        {
            var credential = Marshal.PtrToStructure<NativeCredential>(credentialPointer);
            if (credential.CredentialBlob == IntPtr.Zero || credential.CredentialBlobSize is 0 or > MaximumCredentialBlobBytes)
                throw new InvalidOperationException("저장된 GitHub 인증 정보 형식이 올바르지 않습니다.");

            var blob = new byte[credential.CredentialBlobSize];
            try
            {
                Marshal.Copy(credential.CredentialBlob, blob, 0, blob.Length);
                var token = StrictUtf8.GetString(blob);
                ValidateToken(token);
                return token;
            }
            catch (DecoderFallbackException)
            {
                throw new InvalidOperationException("저장된 GitHub 인증 정보 형식이 올바르지 않습니다.");
            }
            finally
            {
                CryptographicOperations.ZeroMemory(blob);
                var zeroes = new byte[credential.CredentialBlobSize];
                Marshal.Copy(zeroes, 0, credential.CredentialBlob, zeroes.Length);
                CryptographicOperations.ZeroMemory(zeroes);
            }
        }
        finally
        {
            CredFree(credentialPointer);
        }
    }

    public void SaveToken(string token)
    {
        ArgumentNullException.ThrowIfNull(token);
        ValidateToken(token);
        var secret = StrictUtf8.GetBytes(token);
        if (secret.Length > MaximumCredentialBlobBytes)
        {
            CryptographicOperations.ZeroMemory(secret);
            throw new ArgumentException("GitHub 인증 정보가 Windows 자격 증명 저장 한도를 초과합니다.", nameof(token));
        }

        var targetPointer = IntPtr.Zero;
        var credentialPointer = IntPtr.Zero;
        try
        {
            targetPointer = Marshal.StringToHGlobalUni(targetName);
            var credential = new NativeCredential
            {
                Type = GenericCredential,
                TargetName = targetPointer,
                CredentialBlobSize = (uint)secret.Length,
                CredentialBlob = IntPtr.Zero,
                Persist = PersistLocalMachine
            };
            var pinnedSecret = GCHandle.Alloc(secret, GCHandleType.Pinned);
            try
            {
                credential.CredentialBlob = pinnedSecret.AddrOfPinnedObject();
                credentialPointer = Marshal.AllocHGlobal(Marshal.SizeOf<NativeCredential>());
                Marshal.StructureToPtr(credential, credentialPointer, fDeleteOld: false);
                if (!CredWriteW(credentialPointer, 0))
                    throw new InvalidOperationException("Windows 자격 증명에 GitHub 인증 정보를 저장하지 못했습니다.");
            }
            finally
            {
                pinnedSecret.Free();
            }
        }
        finally
        {
            CryptographicOperations.ZeroMemory(secret);
            if (credentialPointer != IntPtr.Zero)
            {
                var zeroes = new byte[Marshal.SizeOf<NativeCredential>()];
                Marshal.Copy(zeroes, 0, credentialPointer, zeroes.Length);
                Marshal.FreeHGlobal(credentialPointer);
                CryptographicOperations.ZeroMemory(zeroes);
            }
            if (targetPointer != IntPtr.Zero) Marshal.FreeHGlobal(targetPointer);
        }
    }

    public void DeleteToken()
    {
        if (CredDeleteW(targetName, GenericCredential, 0)) return;
        if (Marshal.GetLastWin32Error() == ErrorNotFound) return;
        throw new InvalidOperationException("Windows 자격 증명에서 GitHub 인증 정보를 삭제하지 못했습니다.");
    }

    private static void ValidateToken(string token)
    {
        if (string.IsNullOrWhiteSpace(token) || !string.Equals(token, token.Trim(), StringComparison.Ordinal) ||
            token.Any(character => char.IsWhiteSpace(character) || char.IsControl(character)))
            throw new ArgumentException("GitHub 인증 정보를 확인하세요. 앞뒤 공백과 줄바꿈은 허용되지 않습니다.", nameof(token));
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct NativeCredential
    {
        public uint Flags;
        public uint Type;
        public IntPtr TargetName;
        public IntPtr Comment;
        public System.Runtime.InteropServices.ComTypes.FILETIME LastWritten;
        public uint CredentialBlobSize;
        public IntPtr CredentialBlob;
        public uint Persist;
        public uint AttributeCount;
        public IntPtr Attributes;
        public IntPtr TargetAlias;
        public IntPtr UserName;
    }

    [DllImport("advapi32.dll", EntryPoint = "CredReadW", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool CredReadW(string targetName, uint type, uint flags, out IntPtr credential);

    [DllImport("advapi32.dll", EntryPoint = "CredWriteW", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool CredWriteW(IntPtr credential, uint flags);

    [DllImport("advapi32.dll", EntryPoint = "CredDeleteW", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool CredDeleteW(string targetName, uint type, uint flags);

    [DllImport("advapi32.dll", EntryPoint = "CredFree")]
    private static extern void CredFree(IntPtr buffer);
}
