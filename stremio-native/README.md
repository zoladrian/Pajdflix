# Pajdflix — źródła aplikacji

Opis programu, różnice wobec Stremio, instalacja wydania i licencja są w [README repozytorium](../README.md).

To katalog aplikacji. Serwer strumieni leży obok, w `../stream-server`, i jest podpięty w `Cargo.toml`.

Budowanie z tego katalogu, tylko gdy naprawdę chcesz kompilować:

```powershell
.\scripts\Build-Pajdflix.ps1             # szybki profil lokalny
.\scripts\Build-Pajdflix.ps1 -Mode Full  # profil wydania
.\setup\create_setup.cmd                 # wydanie i instalator Windows
```

Instalator powstaje jako `artifacts/PajdflixSetup-v1.0.0-x64.exe`. Dane użytkownika zostają w `%LOCALAPPDATA%\stremio-native`.

Licencja tego katalogu: GPL-3.0-only, tekst w [LICENSE](LICENSE). To zmieniona wersja [Stremio Native](https://github.com/stremio-native/stremio-native).
