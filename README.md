 Pajdflix

**Film z torrenta, po polsku, bez kombinowania.**

Pajdflix jest od tego, ┼╝eby w┼é─ůczy─ç film albo serial i ju┼╝ go ogl─ůda─ç. Nie szukasz pliku, nie por├│wnujesz dziesi─Öciu wyda┼ä i nie sk┼éadasz odtwarzacza. Wybierasz tytu┼é. Program sam znajduje torrent, bierze 1080p, woli polski dubbing, lektor albo napisy i puszcza seans, gdy tylko da si─Ö go wystartowa─ç. Seedowanie zostaje wy┼é─ůczone.

## Jak to dzia┼éa

1. Otwierasz katalog albo wpisujesz tytu┼é.
2. Klikasz **Ogl─ůdaj**.
3. Pajdflix odrzuca zwiastuny i pliki, kt├│re nie wygl─ůdaj─ů na film, i zostawia polsk─ů wersj─Ö.
4. Ekran robi si─Ö pe┼ény. Film idzie od razu, reszta doci─ůga si─Ö w tle.
5. Napisy z polskich dodatk├│w s─ů prostowane, wi─Öc wchodz─ů polskie znaki, a nie pytajniki.
6. Po seansie cache mo┼╝na sprz─ůtn─ů─ç. Zostaje obejrzany film, nie sterta plik├│w.

Ty tylko ogl─ůdasz.

To zmieniona wersja spo┼éeczno┼Ťciowego [Stremio Native](https://github.com/stremio-native/stremio-native). Nie jest aplikacj─ů SmartCode i nie jest oficjalnym Stremio.

## Co jest w ┼Ťrodku

- **Katalog i wyszukiwanie.** Strona g┼é├│wna, karta tytu┼éu, biblioteka, profile i dodatki m├│wi─ůce protoko┼éem Stremio.
- **Odtwarzacz.** Obraz idzie przez mpv, z napisami, ┼Ťcie┼╝kami audio, skalowaniem kadru i trybem pe┼énoekranowym od startu.
- **┼╣r├│d┼éa.** Przycisk ÔÇ×Ogl─ůdajÔÇŁ wybiera jako┼Ť─ç (domy┼Ťlnie sensowne 1080p) i woli polski dubbing, lektor albo napisy. Podejrzane pliki i zwiastuny nie startuj─ů same.
- **Torrenty w tym samym programie.** Pobieranie i odtwarzanie obs┼éuguje wbudowany serwer. Seedowanie jest wy┼é─ůczone.
- **Polskie napisy.** Pliki w Windows-1250, ISO-8859-2 i UTF-16 s─ů zamieniane na UTF-8. Wersja, w kt├│rej polskie litery ju┼╝ znikn─Ö┼éy, jest pomijana.
- **Porz─ůdek na dysku.** Obejrzany cache torrenta da si─Ö sprz─ůtn─ů─ç po seansie. W┼éasne pliki z dysku nie s─ů przy tym ruszane.
- **Obraz.** Na karcie graficznej mo┼╝na w┼é─ůczy─ç filtry Anime4K albo lekki FSR. Domy┼Ťlnie s─ů wy┼é─ůczone.

## Czym to si─Ö r├│┼╝ni od Stremio

Oficjalne Stremio na komputer to interfejs webowy w oknie aplikacji i osobny serwer. Pajdflix jest jednym programem napisanym w Rust, z w┼éasnym interfejsem.

| | Stremio | Pajdflix |
|---|---|---|
| Okno | strona w os┼éonie desktopowej | natywny interfejs |
| Serwer strumieni | osobny proces Node.js | wbudowany, w tym samym programie |
| J─Özyk | wiele, interfejs webowy | polski interfejs od pierwszej sekundy |
| Start | okno w rozmiarze pulpitu | pe┼ény ekran |
| Konto i synchronizacja Stremio | tak | nie polegamy na chmurze Stremio; biblioteka jest lokalna |
| Seedowanie | zale┼╝y od ustawie┼ä | wy┼é─ůczone |
| Napisy polskie | jak je odda dodatek | dodatkowo prostowane kodowanie |
| Oficjalna aplikacja SmartCode | tak | nie |

Dodatki, kt├│rych u┼╝ywasz w Stremio, da si─Ö pod┼é─ůczy─ç tutaj, je┼Ťli m├│wi─ů tym samym protoko┼éem. Katalog Cinemeta, Torrentio czy dodatki z napisami s─ů osobnymi us┼éugami. Pajdflix ich nie zawiera i nie utrzymuje.

Cz─Ö┼Ťci oficjalnego Stremio tu nie ma: synchronizacji konta mi─Ödzy telefonem i komputerem, aplikacji mobilnej, telewizora ani sklepu dodatk├│w w tej samej postaci. To odtwarzacz na Windows.

## Instalacja

Na Windows 10 lub 11, 64-bit. Procesor musi obs┼éugiwa─ç zestaw x86-64-v3 (AVX2). To praktycznie ka┼╝dy komputer z ostatnich lat. Do interfejsu potrzebny jest sterownik z OpenGL.

1. Otw├│rz [wydania Pajdflix](https://github.com/zoladrian/Pajdflix/releases).
2. Przy najnowszej wersji pobierz instalator `PajdflixSetup-vÔÇŽ-x64.exe`. Pierwsze wydanie z tych ┼║r├│de┼é to **1.0.0**, plik `PajdflixSetup-v1.0.0-x64.exe`.
3. Uruchom go. Instalacja jest dla bie┼╝─ůcego u┼╝ytkownika i nie prosi o uprawnienia administratora.
4. Zaznacz skr├│t na pulpicie, je┼Ťli chcesz mie─ç ikon─Ö pod r─Ök─ů. Ta opcja domy┼Ťlnie nie jest w┼é─ůczona.
5. Na ko┼äcu kreatora mo┼╝esz od razu uruchomi─ç Pajdflix.

Program l─ůduje w `%LOCALAPPDATA%\Programs\stremio-native`. Biblioteka, cache i logi zostaj─ů w `%LOCALAPPDATA%\stremio-native`, tak┼╝e po zdj─Öciu programu. Dzi─Öki tej ┼Ťcie┼╝ce starszy profil nie ginie przy kolejnej instalacji.

Aktualizacja to ponowne uruchomienie nowszego instalatora z tej samej strony wyda┼ä. Pajdflix nie pobiera wyda┼ä obcego projektu jako w┼éasnych aktualizacji.

Deinstalacja jest w Ustawieniach Windows, przy aplikacjach, pod nazw─ů Pajdflix.


Aplikacja na Androida (osobne wydanie APK): [android-v1.0.13-mvp](https://github.com/zoladrian/Pajdflix/releases/tag/android-v1.0.13-mvp).
## Licencja

Pajdflix jest wolnym oprogramowaniem na **GNU GPL w wersji 3**. Mo┼╝esz go u┼╝ywa─ç, zmienia─ç i rozdawa─ç, tak┼╝e pod inn─ů nazw─ů, pod warunkiem ┼╝e odbiorca dostaje t─Ö sam─ů licencj─Ö i dost─Öp do ┼║r├│de┼é dok┼éadnie tej wersji. ┼╣r├│d┼éa wydania s─ů w tym repozytorium.

Nie wolno zamkn─ů─ç tej ca┼éo┼Ťci ani og┼éosi─ç jej w┼éasno┼Ťci─ů jednej osoby. Kod Stremio Native nale┼╝y do jego autor├│w. Zmiany Pajdflix s─ů dok┼éadane na tej samej licencji.

Szczeg├│┼éy i licencje cz─Ö┼Ťci sk┼éadowych s─ů w [NOTICE.md](NOTICE.md).

Nazwa i znak Stremio nale┼╝─ů do SmartCode Ltd. Pajdflix nie jest przez nich prowadzony, sponsorowany ani zatwierdzony.
