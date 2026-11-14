using System;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Windows.Forms;

internal static class PajdflixLauncher
{
    internal static string ResolveTarget(string repository)
    {
        var roots = new[] { Path.Combine(repository, "target"), Path.Combine(repository, "target", "x86_64-pc-windows-msvc") };
        return roots.SelectMany(root => new[] { "release", "local", "debug" }.Select(profile => Path.Combine(root, profile, "stremio-native.exe")))
            .Where(IsReady)
            .OrderByDescending(path => SnapshotTime(path).HasValue)
            .ThenByDescending(path => SnapshotTime(path) ?? File.GetLastWriteTimeUtc(path))
            .ThenBy(path => path.IndexOf("release", StringComparison.OrdinalIgnoreCase) >= 0 ? 0 : 1)
            .FirstOrDefault();
    }

    private static DateTime? SnapshotTime(string path)
    {
        try
        {
            const string prefix = "PajdflixSourceTime=";
            string comment = FileVersionInfo.GetVersionInfo(path).Comments;
            long milliseconds;
            if (comment != null && comment.StartsWith(prefix, StringComparison.Ordinal)
                && Int64.TryParse(comment.Substring(prefix.Length), out milliseconds))
                return new DateTime(1970, 1, 1, 0, 0, 0, DateTimeKind.Utc).AddMilliseconds(milliseconds);
        }
        catch (Exception) { /* Older or incomplete build metadata: use file time. */ }
        return null;
    }

    private static bool IsReady(string path)
    {
        try
        {
            if (!File.Exists(Path.Combine(Path.GetDirectoryName(path), "libmpv-2.dll"))) return false;
            // Reject missing files and executables still being written by the build.
            using (var input = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read))
                return input.Length > 2 && input.ReadByte() == 'M' && input.ReadByte() == 'Z';
        }
        catch (IOException) { return false; }
        catch (UnauthorizedAccessException) { return false; }
    }

    [STAThread]
    private static int Main(string[] args)
    {
        try
        {
            string directory = AppDomain.CurrentDomain.BaseDirectory;
            string repository = File.ReadAllText(Path.Combine(directory, "repository.txt")).Trim();
            string selected = ResolveTarget(repository);
            if (selected == null) throw new InvalidOperationException("Nie znaleziono gotowej wersji Pajdflix. Najpierw zbuduj aplikację w tym projekcie.");
            File.WriteAllText(Path.Combine(directory, "last-selection.txt"), DateTime.UtcNow.ToString("o") + Environment.NewLine + selected);
            if (args.Length == 1 && args[0] == "--resolve-only") return 0;

            foreach (var process in Process.GetProcessesByName("stremio-native"))
            {
                using (process)
                {
                    try
                    {
                        string running = process.MainModule.FileName;
                        if (running.StartsWith(Path.Combine(repository, "target") + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase)
                            && (!String.Equals(running, selected, StringComparison.OrdinalIgnoreCase)
                                || process.StartTime.ToUniversalTime().AddSeconds(2) < File.GetLastWriteTimeUtc(selected)))
                        {
                            MessageBox.Show("Dostępna jest nowsza wersja Pajdflix. Zamknij działającą aplikację (również w zasobniku obok zegara) i kliknij skrót ponownie.", "Pajdflix", MessageBoxButtons.OK, MessageBoxIcon.Information);
                            return 2;
                        }
                    }
                    catch (System.ComponentModel.Win32Exception) { }
                    catch (InvalidOperationException) { }
                }
            }

            Process.Start(new ProcessStartInfo(selected) { WorkingDirectory = Path.GetDirectoryName(selected), UseShellExecute = true });
            return 0;
        }
        catch (Exception error)
        {
            MessageBox.Show(error.Message, "Pajdflix — nie można uruchomić", MessageBoxButtons.OK, MessageBoxIcon.Error);
            return 1;
        }
    }
}
