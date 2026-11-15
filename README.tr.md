<p align="center">
  <img src="assets/logo.png" width="120" alt="Tuxius logosu">
</p>

<h1 align="center">Tuxius</h1>

<p align="center">
  <b>SSH, yazmadan.</b><br>
  Sunucularını bir kez kaydet. <code>tuxius</code> yaz, seç, bağlan.
</p>

<p align="center">
  <a href="https://erdinckaya97.github.io/tuxius">Web sitesi</a> ·
  <a href="#kurulum">Kurulum</a> ·
  <a href="#kullanım">Kullanım</a> ·
  <a href="#güvenlik">Güvenlik</a> ·
  <a href="README.md">English</a>
</p>

<p align="center">
  <img src="assets/demo.gif" alt="Tuxius demo" width="760">
</p>

---

Her seferinde `ssh root@203.0.113.10 -p 2222` yazıp şifre aramaktan sıkıldın mı? Tuxius terminalde yaşayan küçük ve hızlı bir SSH yöneticisi: sunucularını bir kez kaydet, tek tuşla bağlan. Tek bir küçük dosya; hesap, bulut ya da arayüz yok.

- **Tek komut** — `tuxius` yaz, sunucu listen gelsin. Ok tuşları, Enter, içeridesin.
- **İlk seferden sonra şifresiz** — root şifresini bir kez gir; Tuxius sunucuya SSH anahtarı kurar ve şifreyi hiçbir yere kaydetmez.
- **Anlık arama** — isim ya da IP yazmaya başla, liste filtrelensin.
- **Doğrudan bağlan** — `tuxius web` direkt bağlanır (benzersiz kısaltmalar da olur).
- **Uzaktan komut** — `tuxius web -- df -h`.
- **Kendi SSH anahtarların** — mevcut anahtarlarını içe aktar, yenisini üret, sil. Sunucuyu silerken anahtarını sunucudan da kaldırabilirsin.
- **Bağımlılık yok** — sistemindeki OpenSSH'ı kullanan ~1 MB'lık tek Rust dosyası.

## Kurulum

```sh
curl -fsSL https://raw.githubusercontent.com/erdinckaya97/tuxius/main/install.sh | sh
```

**Linux** (x86_64, arm64) ve **macOS** (Intel, Apple Silicon) desteklenir.

Cargo ile: `cargo install --git https://github.com/erdinckaya97/tuxius`

Kaldırmak için:

```sh
sudo rm /usr/local/bin/tuxius        # veya ~/.local/bin/tuxius
rm -rf ~/.config/tuxius
```

## Kullanım

```sh
tuxius
```

İlk açılışta Tuxius sunucu eklemen için adım adım sorar: **isim**, **adres**, **kullanıcı** (varsayılan `root`), **port** (varsayılan `22`), **giriş yöntemi** ve **şifre**. Sonra `tuxius` yazdığında kayıtlı sunucuların gelir; en son kullandığın en üstte.

| Komut | Ne yapar |
| --- | --- |
| `tuxius` | Sunucu listesini açar |
| `tuxius <isim>` | Doğrudan bağlanır |
| `tuxius <isim> -- <komut>` | Sunucuda komut çalıştırır |
| `tuxius add` | Yeni sunucu kaydeder |
| `tuxius list` | Kayıtlı sunucuları listeler |
| `tuxius edit [isim]` | Sunucuyu düzenler |
| `tuxius rm [isim]` | Sunucuyu siler (`--revoke` anahtarı sunucudan da kaldırır) |
| `tuxius key` | SSH anahtar yöneticisini açar |
| `tuxius key list` | Anahtarları ve hangi sunucuların kullandığını listeler |
| `tuxius key add` | Anahtar dosyası içe aktarır (`--file ~/.ssh/id_ed25519`) ya da yenisini üretir (`--generate`) |
| `tuxius key show [isim]` | Açık anahtarı yazdırır (bulut paneline yapıştırmak için) |
| `tuxius key rm [isim]` | Anahtarı siler (bir sunucu kullanıyorsa izin vermez) |
| `tuxius path` | Dosyaların nerede durduğunu gösterir |

### Giriş yöntemleri

- **Yeni Tuxius anahtarı (önerilen)** — şifreyi bir kez girersin, Tuxius sunucuya kendi anahtarını kurar.
- **Kendi SSH anahtarım** — kayıtlı bir anahtar seç ya da o an içe aktar / üret. Sunucu henüz kabul etmiyorsa Tuxius şifreyle kurabilir.
- **Kayıtlı şifre** — anahtara izin vermeyen sunucular için.

Anahtarlar `~/.config/tuxius/keys/` altında (chmod 600) durur. Menüden (`⚿ SSH keys`) ya da `tuxius key` ile yönetilir.

## Güvenlik

- **SSH anahtarı (varsayılan, önerilen).** Tuxius `~/.config/tuxius/keys/tuxius` adında kendi anahtarını oluşturur, şifreyi **bir kez** kullanarak açık anahtarı sunucunun `~/.ssh/authorized_keys` dosyasına ekler. Şifre diske yazılmaz.
- **Kayıtlı şifre (isteğe bağlı).** Sunucu anahtar kabul etmiyorsa şifreyi kaydetmeyi seçebilirsin. `~/.config/tuxius/servers.toml` dosyasında, sadece senin okuyabileceğin şekilde (`chmod 600`) saklanır. Şifrelenmez — mümkünse anahtar kullan.
- Şifre OpenSSH'a `SSH_ASKPASS` üzerinden verilir, komut satırında görünmez.

## Lisans

[MIT](LICENSE)
