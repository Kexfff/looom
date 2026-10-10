# Полная установка looom: Intel N100 и отдельный диск

Для автоматической установки доступен [экспериментальный нативный мастер](installer.md).
Ниже сохранён **исторический GRUB-путь** для диагностики исходного пилота.
Для новой установки использовать мастер с Limine; дальнейшую работу и переход
со старой версии описывает [документация Limine](limine.md).
Исторические команды GRUB ниже не применяются поверх новой Limine-установки.

Сценарий: Arch ISO → обычный Arch в `@bootstrap` → клонирование GitHub и сборка
Rust → looom → read-only релиз с KDE Plasma. Пользователь выбрал целый диск;
процессор предположительно Intel N100, модель проверяется перед установкой.
Команды выполняются **на устанавливаемом компьютере**, а не на машине,
с которой ты читаешь инструкцию.

**Раздел 3 удаляет всё содержимое выбранного диска.** Для сохранения старой ОС,
dual boot или использования существующей ESP эту разметку не применять.
Пилот: x86_64, UEFI, Secure Boot выключен, без шифрования, GRUB, один Btrfs.
Механика проверена в VM и на физическом MINI S с Intel N100: два ядра,
Plasma Wayland с Intel GPU, откаты и один короткий цикл S3/RTC на LTS.
Это не заменяет приёмку другого оборудования.
[Аппаратный журнал и ограничения](runs/2026-10-07-n100/README.md).
[Контракт](native-rust.md), [журнал VM](runs/2026-10-06-native-rust/README.md).

## 1. Флешка, прошивка и сеть

Скачать ISO с [официальной страницы Arch](https://archlinux.org/download/),
проверить опубликованные checksum/signature и записать ISO на USB средствами
своей ОС. Для первого пилота удобны Ethernet, 8 GiB RAM и SSD от 80 GiB.
Это практический запас для сборок, не проверенные минимальные требования.

В прошивке отключить Secure Boot, выбрать UEFI и загрузить USB именно в UEFI.
По возможности временно отключить остальные диски, чтобы не спутать целевой
диск с дисками данных. После загрузки Arch ISO ты находишься в root shell:

```bash
test -d /sys/firmware/efi/efivars && echo 'UEFI OK'
lsblk -o NAME,PATH,SIZE,MODEL,SERIAL,TYPE,FSTYPE,MOUNTPOINTS
lscpu
lspci -nnk
ip -br address
timedatectl set-ntp true
curl -I https://archlinux.org
```

Если `UEFI OK` не появился, перезагрузить USB в UEFI. Проверить, что CPU
действительно Intel N100 и графика Intel. Если модель другая, пересмотреть
microcode/драйверы до сборки. Пароли Wi-Fi не передавать аргументами команд.
Для Wi-Fi в Live ISO используется iwd:

```text
iwctl
device list
station wlan0 scan
station wlan0 get-networks
station wlan0 connect "ИМЯ_СЕТИ"
exit
```

`wlan0` заменить на имя из device list. Соединение Live ISO не переносится
в установленную систему: его сохраним через NetworkManager после первой загрузки.

## 2. Выбрать диск и дату пакетов

Пример ниже — **`/dev/nvme0n1`**, не автоматически выбранный диск.
Сверить объём, модель и серийный номер из lsblk.

| Тип | Диск | ESP | Btrfs |
| --- | --- | --- | --- |
| NVMe | `/dev/nvme0n1` | `/dev/nvme0n1p1` | `/dev/nvme0n1p2` |
| SATA/SCSI | `/dev/sda` | `/dev/sda1` | `/dev/sda2` |
| VirtIO VM | `/dev/vda` | `/dev/vda1` | `/dev/vda2` |

Задать свои значения в одном Live shell:

```bash
looom_disk=/dev/nvme0n1
looom_esp=/dev/nvme0n1p1
looom_btrfs=/dev/nvme0n1p2
looom_archive=2026/10/03
lsblk -o NAME,SIZE,MODEL,SERIAL,FSTYPE,MOUNTPOINTS "$looom_disk"
```

Сверить MOUNTPOINTS **всех разделов** целевого диска. Если они заняты или
на них есть данные, которые надо сохранить — остановиться. Дальше таблица
разделов и всё содержимое целевого диска удаляются.

`2026/10/03` — архив, использованный в VM. Проверить его доступность:

```bash
curl --fail --head "https://archive.archlinux.org/repos/$looom_archive/core/os/x86_64/core.db"
```

Если дата недоступна, не подменять её незаметно rolling-зеркалом. Другую дату
нужно одинаково выбрать для bootstrap и YAML, затем получить свой lock.
Обновление всей даты архива пока не проходило отдельную приёмку.

## 3. GPT и форматирование

**Всё на `$looom_disk` будет удалено.** Выполнять после выбора диска выше:

```bash
sgdisk --zap-all "$looom_disk"
sgdisk --new=1:0:+2G --typecode=1:ef00 --change-name=1:LOOOM-ESP \
       --new=2:0:0 --typecode=2:8300 --change-name=2:LOOOM-ROOT "$looom_disk"
partprobe "$looom_disk"
udevadm settle
lsblk -o NAME,SIZE,PARTTYPE,FSTYPE,MOUNTPOINTS "$looom_disk"
mkfs.fat -F 32 -n LOOOM_EFI "$looom_esp"
mkfs.btrfs -L LOOOM_ROOT "$looom_btrfs"
```

Раздел 1 — FAT32 ESP 2 GiB. Раздел 2 — Btrfs на всё остальное.
Swap-раздел и гибернация сейчас не настраиваются: дополнительные записи fstab
не входят в текущий контракт. Zram можно добавить отдельным изменением позже.

## 4. Btrfs-подтомы и монтирования

```bash
mkdir -p /mnt/looom-top
mount -o subvolid=5,compress=zstd:3 "$looom_btrfs" /mnt/looom-top
for looom_subvol in @bootstrap @home @var @state; do
    btrfs subvolume create "/mnt/looom-top/$looom_subvol"
done
umount /mnt/looom-top
mount -o subvol=@bootstrap,noatime,compress=zstd:3 "$looom_btrfs" /mnt
mkdir -p /mnt/home /mnt/var /mnt/efi
mount -o subvol=@home,noatime,compress=zstd:3 "$looom_btrfs" /mnt/home
mount -o subvol=@var,noatime,compress=zstd:3 "$looom_btrfs" /mnt/var
mkdir -p /mnt/var/lib/looom
mount -o subvol=@state,noatime,compress=zstd:3 "$looom_btrfs" /mnt/var/lib/looom
chmod 0700 /mnt/var/lib/looom
mount -o umask=0077 "$looom_esp" /mnt/efi
findmnt -R /mnt
```

| Подтом/раздел | Монтирование | Назначение |
| --- | --- | --- |
| `@bootstrap` | `/mnt` | обычный writable Arch и аварийная база |
| `@home` | `/mnt/home` | личные данные |
| `@var` | `/mnt/var` | журналы, кэш, данные служб |
| `@state` | `/mnt/var/lib/looom` | менеджер, конфигурация и credentials |
| FAT ESP | `/mnt/efi` | загрузчики, меню и UKI |

Все четыре подтома — на **верхнем уровне одного Btrfs**, без вложения друг в
друга. State монтируется после var. Подтомы не имеют фиксированных долей диска:
используют общий свободный объём. Root/build подтомы потом создаёт looom.
Не устанавливать пакеты, пока findmnt не показывает эту раскладку.

## 5. Установить Arch из выбранного архива

В heredoc `$looom_archive` подставляется, а `\$repo`/`\$arch` остаются для pacman:

```bash
cat > /tmp/looom-pacman.conf <<EOF
[options]
Architecture = auto
CheckSpace
ParallelDownloads = 5
SigLevel = Required DatabaseOptional
LocalFileSigLevel = Required
[core]
Server = https://archive.archlinux.org/repos/$looom_archive/\$repo/os/\$arch
[extra]
Server = https://archive.archlinux.org/repos/$looom_archive/\$repo/os/\$arch
EOF
pacstrap -K -M -C /tmp/looom-pacman.conf /mnt \
  base linux linux-firmware intel-ucode btrfs-progs dosfstools gptfdisk grub \
  efibootmgr mkinitcpio systemd-ukify arch-install-scripts openssh sudo \
  networkmanager curl git vim pciutils rust base-devel
cp /tmp/looom-pacman.conf /mnt/etc/pacman.conf
genfstab -U /mnt > /mnt/etc/fstab
cat /mnt/etc/fstab
```

Пять монтирований из таблицы должны быть в fstab. Root содержит
`subvol=/@bootstrap` или `subvol=@bootstrap`, ESP — umask=0077.
Используем `>` один раз, чтобы не дублировать строки повторным `>>`.
Если pacstrap завершился ошибкой, исправить её до перехода к следующему шагу.

## 6. Настроить bootstrap в chroot

```bash
arch-chroot /mnt
```

Теперь команды выполняются **внутри установленного Arch**. Live-переменные сюда
не переносятся. Пример: пользователь `owner`, UID/GID 1000, hostname `looom-pc`,
timezone Europe/Moscow. Можно выбрать свои значения и затем повторить их в YAML.
Имя пользователя — строчные буквы, цифры и дефис, не root.

```bash
ln -sf /usr/share/zoneinfo/Europe/Moscow /etc/localtime
hwclock --systohc
printf 'en_US.UTF-8 UTF-8\nru_RU.UTF-8 UTF-8\n' > /etc/locale.gen
locale-gen
printf 'LANG=en_US.UTF-8\n' > /etc/locale.conf
printf 'KEYMAP=us\n' > /etc/vconsole.conf
printf 'looom-pc\n' > /etc/hostname
printf '127.0.0.1 localhost\n::1 localhost\n127.0.1.1 looom-pc\n' > /etc/hosts
systemd-machine-id-setup
groupadd -g 1000 owner
useradd -m -u 1000 -g 1000 -G wheel -s /bin/bash owner
passwd
passwd owner
printf '%%wheel ALL=(ALL:ALL) ALL\n' > /etc/sudoers.d/10-wheel
chmod 0440 /etc/sudoers.d/10-wheel
visudo -cf /etc/sudoers
install -d -m 0700 /etc/looom-local /etc/NetworkManager/system-connections
systemctl enable NetworkManager systemd-timesyncd
ssh-keygen -A
```

Root и пользователю нужны пароли: текущий импорт не поддерживает empty/locked
root hash. Не выводить shadow или password hashes в журнал/YAML.
После перехода в read-only систему смена пароля выполняется через looom.

Перенести базу pacman bootstrap в его собственный root, чтобы общий `/var`
не смешивал сведения о пакетах разных релизов:

```bash
install -d /usr/lib/looom
mv /var/lib/pacman /usr/lib/looom/pacman
ln -s /usr/lib/looom/pacman /var/lib/pacman
sed -i '/^\[options\]/a DBPath = /usr/lib/looom/pacman' /etc/pacman.conf
pacman -Dk
```

Это перенос **один раз**. Каждый новый root имеет свою базу по тому же пути;
симлинк в общем var разрешается относительно текущего корня.
Intel microcode уже установлен. Plasma-профиль включает Mesa; модель GPU и
реальное ускорение проверим после загрузки. Bluetooth пакеты и unit, если
нужны, объявляются отдельно: bluez/bluez-utils и bluetooth.service.
У [Intel N100](https://www.intel.com/content/www/us/en/products/sku/231803/intel-processor-n100-6m-cache-up-to-3-40-ghz/specifications.html)
предусмотрена Intel UHD Graphics; установленное устройство всё равно сверяем
через lspci, чтобы не принять предположение о модели за проверку компьютера.

## 7. GRUB и первая обычная загрузка

Сначала проверить обычный writable Arch. Его загрузчик `ArchBootstrap`
остаётся запасным; позже looom создаёт отдельную UEFI-запись.

```bash
looom_kernel_dir=$(find /usr/lib/modules -mindepth 1 -maxdepth 1 -type d)
test -f "$looom_kernel_dir/vmlinuz"
install -m 0644 "$looom_kernel_dir/vmlinuz" /boot/vmlinuz-linux
cat > /etc/mkinitcpio.conf <<'EOF'
MODULES=(btrfs)
BINARIES=()
FILES=()
HOOKS=(base systemd microcode modconf kms keyboard sd-vconsole block filesystems fsck)
COMPRESSION="zstd"
EOF
cat > /etc/mkinitcpio.d/linux.preset <<'EOF'
ALL_config="/etc/mkinitcpio.conf"
ALL_kver="/boot/vmlinuz-linux"
PRESETS=('default')
default_image="/boot/initramfs-linux.img"
EOF
mkinitcpio -P
cat > /etc/default/grub <<'EOF'
GRUB_DEFAULT=0
GRUB_TIMEOUT=5
GRUB_DISTRIBUTOR="Arch"
GRUB_CMDLINE_LINUX="rootflags=subvol=@bootstrap"
GRUB_CMDLINE_LINUX_DEFAULT="loglevel=3"
EOF
grub-install --target=x86_64-efi --efi-directory=/efi --bootloader-id=ArchBootstrap
grub-mkconfig -o /boot/grub/grub.cfg
efibootmgr
exit
```

Если kernel-файл, mkinitcpio или grub-install завершились ошибкой, исправить её
до reboot. Широкий initramfs может предупреждать о firmware для отсутствующих
устройств; firmware для своих GPU/сети должно быть доступно.
Теперь снова Live ISO:

```bash
umount -R /mnt
reboot
```

Извлечь USB или выбрать ArchBootstrap в boot menu. Войти в установленный Arch
локально как root и проверить обычную базовую систему:

```bash
findmnt -nro FSTYPE,FSROOT,OPTIONS /
findmnt -R /
ip -br address
timedatectl status
```

Root должен быть `@bootstrap`; home/var/state/efi должны быть отдельными mounts.
Для Wi-Fi в **установленном Arch** сохранить соединение:

```bash
nmcli device status
nmcli device wifi list
nmcli --ask device wifi connect "ИМЯ_СЕТИ"
curl -I https://github.com
```

Ethernet обычно подключается автоматически. NetworkManager credentials
сохранятся при переключениях root после подключения looom.

## 8. Клонирование GitHub и сборка Rust

В загруженном writable bootstrap от root:

```bash
git clone https://github.com/Kexfff/looom.git /var/lib/looom/src
cd /var/lib/looom/src
git rev-parse HEAD
cargo build --locked --release --bin looom
install -m 0755 target/release/looom /usr/bin/looom
looom --version
```

Для Cargo нужен интернет. Rust/base-devel установлены в bootstrap на шаге 5;
в новый desktop root компилятор автоматически не включается. Не запускать
`cargo test` на реальном компьютере: runtime tests предназначены только для
выделенной VM и защищены её идентификатором. На N100 сборка может занять время;
при нехватке памяти повторить с `CARGO_BUILD_JOBS=1 cargo build --locked --release --bin looom`.

Сохранить commit SHA. Репозиторий не содержит образ VM или ключи доступа.
Sample lock зависит от SHA бинарника; после собственной сборки получить свой lock.

## 9. Своя декларация

```bash
install -d -m 0700 /var/lib/looom/config/desktop
cp configs/native/desktop/base.yaml /var/lib/looom/config/desktop/base.yaml
vim /var/lib/looom/config/desktop/base.yaml
```

Полный пример для подтверждённого Intel N100 и пользователя owner:

```yaml
schema: 1
source:
  profile: arch
  snapshot: "2026-10-03"
system:
  hostname: looom-pc
  timezone: Europe/Moscow
  locale: en_US.UTF-8
  console_keymap: us
kernel:
  package: linux
desktop:
  environment: plasma
packages:
  - intel-ucode
  - pciutils
accounts:
  user:
    name: owner
    uid: 1000
    gid: 1000
    groups: [wheel]
    shell: /bin/bash
    password_secret: login-owner
  root:
    password_secret: login-root
units: {}
files: {}
persistent:
  directories:
    /etc/NetworkManager/system-connections:
      id: network-connections
      owner: root
      group: root
      mode: "0700"
    /etc/looom-local:
      id: local-settings
      owner: root
      group: root
      mode: "0700"
health:
  required_units:
    - NetworkManager.service
```

Имя/UID/GID совпадают с созданным пользователем. password_secret — ссылка,
не пароль. snapshot совпадает с bootstrap. Если CPU AMD, microcode меняется
на amd-ucode и в исходной установке, и здесь; NVIDIA требует отдельного профиля.
Пустой units не отключает инфраструктуру модулей: NetworkManager, аккаунты,
SDDM и графическая цель настраиваются менеджером.

Для SSH объявить `sshd.service: enabled` в units и при необходимости в health.
В релизе SSH принимает только ключи. До первой загрузки положить свой **публичный**
ключ в `/home/owner/.ssh/authorized_keys`, UID/GID 1000:1000, каталог 0700,
файл 0600. Root SSH, если нужен, подготовить в `/root/.ssh` до bootstrap.
Приватный ключ остаётся на клиенте.

На проверенном MINI S новые NvPCR-службы systemd 262 падают из-за недоступных
TPM NV indices. В его профиле без Secure Boot/шифрования объявлены:

```yaml
units:
  sshd.service: enabled
  systemd-tpm2-setup-early.service: masked
  systemd-pcrproduct.service: masked
  systemd-pcrlogin@.service: masked
```

Это наблюдённая особенность конкретного пилота, а не требование для любого N100.
Использовать [его A-профиль](../configs/physical/n100-a/base.yaml) после сверки
пользователя и оборудования. Новый Rust builder переносит запрет запуска
masked-служб в initramfs; простого изменения основного root недостаточно.
Первоначальный emergency bootstrap этого запуска создавался до обнаружения
ошибки; его образ отдельно пересобран с маской, а исходный образ сохранён
в закрытом журнале ремонта. Подробности и процедура — в аппаратном журнале.
**Не запускать `scripts/install-bootstrap.sh` на железе:** он предназначен
только для исторической disposable VM.

## 10. Подключение и проверка аварийной загрузки

```bash
looom check /var/lib/looom/config/desktop/base.yaml
looom bootstrap /var/lib/looom/config/desktop/base.yaml
looom boot-entry
looom status
efibootmgr
systemctl reboot
```

Bootstrap сохраняет первоначальный fstab, credentials и локальные данные,
создаёт looom-bootstrap UKI и отдельный GRUB. Boot-entry регистрирует looom
в UEFI и меняет BootOrder. ArchBootstrap остаётся отдельной записью.
Старый GRUB — `/boot/grub`, новый — `/efi/looom/grub`.
Не выполнять grub-mkconfig поверх меню looom.

Выбрать looom в прошивке, если это не произошло автоматически. Пока релизов
нет, меню looom загружает writable @bootstrap. После входа:

```bash
findmnt -nro FSROOT /
efibootmgr
looom init /var/lib/looom/config/desktop/base.yaml
```

Ожидаются `/@bootstrap`, BootCurrent записи looom и `Machine already initialized`.
Verify предназначен для релизов, не для bootstrap. После регистрации не менять
имя/UID/GID пользователя или UUID/пути записанного machine profile.
Если первый bootstrap прервался, сохранить ошибку и повторить ту же команду
с той же декларацией и mounts: новый native bootstrap продолжает закрытый
журнал. Credentials и EFI namespace не удалять. Старые незавершённые установки
без журнала, несовпадающие UUID/declaration или изменённые credential hashes
требуют отдельного разбора.

## 11. Первый read-only KDE-релиз

В загруженном bootstrap от root:

```bash
looom lock /var/lib/looom/config/desktop/base.yaml
looom plan /var/lib/looom/config/desktop/base.yaml
df -h /efi /var/lib/looom
looom build /var/lib/looom/config/desktop/base.yaml pc-1
looom publish pc-1
looom try pc-1
systemctl reboot
```

Desktop build требует минимум 10 GiB свободного места; нужен запас для новых
корней и UKI. План из bootstrap показывает переход от минимального Arch к KDE.
Lock фиксирует пакеты, подписи, репозитории и рецепт. ID уникален: после
неудачного опыта не использовать pc-1 для других входов.

Build создаёт private корень, проверяет и финализирует его в read-only @root-pc-1.
Publish добавляет UKI/меню, try выбирает **один** пробный старт; saved остаётся
bootstrap. При ошибке остановиться на ней, не переходить к следующей команде
и reboot. Recover восстанавливает настроенный Rust build, но не исправляет
автоматически любую раннюю ошибку пакетной установки.

## 12. Проверить железо и подтвердить

Войти личным пользователем в SDDM. В терминале Plasma:

```bash
cat /etc/looom/release-id
findmnt -nro FSROOT,OPTIONS /
sudo looom verify
sudo looom status
lscpu
lspci -nnk
systemctl --failed
systemctl --user --failed
loginctl show-session "$XDG_SESSION_ID" -p Type -p Active
```

Ожидаются pc-1, `/@root-pc-1`, ro, успешный verify и Wayland-сессия.
Для Intel проверить, что устройство GPU использует подходящий драйвер ядра,
а в настройках Plasma виден ожидаемый графический backend/renderer.
Проверить сеть/Wi-Fi, звук/микрофон, внешний экран, USB, Bluetooth если объявлен,
сон и пробуждение. Home/var/state/ESP writable. Не устанавливать системные
пакеты через pacman -S в действующий read-only root.

После успешной приёмки:

```bash
sudo looom confirm
sudo systemctl reboot
```

Обычный старт должен снова привести в pc-1. До confirm следующий обычный
старт возвращается к saved bootstrap. Если root не загрузился или Plasma
непригодна, перезагрузить и выбрать bootstrap; такой релиз не подтверждать.

## 13. Изменения, пароли, откат

Создать маркер в home и от root в `/etc/looom-local`. Для пароля:

```bash
sudo looom password owner
```

Он вводится интерактивно, хранится вне root и переживает откат.
Стандартные passwd/usermod и GUI-смена в read-only системе пока не поддерживаются.

Добавить tree в packages своей декларации и выполнить:

```bash
sudo looom lock /var/lib/looom/config/desktop/base.yaml
sudo looom plan /var/lib/looom/config/desktop/base.yaml
sudo looom build /var/lib/looom/config/desktop/base.yaml pc-2
sudo looom publish pc-2
sudo looom try pc-2
sudo systemctl reboot
# После входа в pc-2 и проверки:
sudo looom verify
sudo looom confirm
sudo looom rollback pc-1
sudo systemctl reboot
```

В pc-1 tree отсутствует, маркеры и **новый пароль** сохраняются.
Возврат: `sudo looom rollback pc-2`, затем reboot. YAML остаётся на state и может
описывать pc-2 при загруженном pc-1 — plan покажет различие. Форматы данных
var не откатываются вместе с базой.

Очистка сначала показывает кандидатов:

```bash
sudo looom gc --keep 2
# Только после просмотра списка:
sudo looom gc --keep 2 --apply
```

Running/saved/next и минимум два последних подтверждённых релиза защищены.
Frozen inputs, credentials и общие данные не удаляются. После прерывания
журналируемой операции выполнить recover и проверить status.

## 14. Восстановление с Live USB

Сначала попробовать firmware boot menu: looom bootstrap или ArchBootstrap.
Если это не помогает, загрузить ISO в UEFI и **не выполнять форматирование**.
Определить свои существующие разделы через lsblk/blkid, затем:

```bash
looom_esp=/dev/nvme0n1p1
looom_btrfs=/dev/nvme0n1p2
mount -o subvol=@bootstrap,compress=zstd:3 "$looom_btrfs" /mnt
mount -o subvol=@home "$looom_btrfs" /mnt/home
mount -o subvol=@var "$looom_btrfs" /mnt/var
mount -o subvol=@state "$looom_btrfs" /mnt/var/lib/looom
mount -o umask=0077 "$looom_esp" /mnt/efi
arch-chroot /mnt
```

После bootstrap есть три дополнительных bind-mount. В аварийном chroot:

```bash
mount --bind /var/lib/looom/local-etc/looom-local /etc/looom-local
mount --bind /var/lib/looom/local-etc/NetworkManager/system-connections \
  /etc/NetworkManager/system-connections
mount --bind /var/lib/looom/root-ssh /root/.ssh
looom status
```

Для прежнего **подтверждённого** релиза использовать looom rollback pc-1.
Bootstrap не является обычным release ID; для выбора writable bootstrap:

```bash
grub-editenv /efi/looom/grub/grubenv set saved_entry=looom-bootstrap
grub-editenv /efi/looom/grub/grubenv unset next_entry
grub-editenv /efi/looom/grub/grubenv list
exit
umount -R /mnt
reboot
```

Chroot не заменяет настоящую загрузку: verify/confirm там не выполнять.
Перед umount -R убедиться, что looom status/rollback не оставили внутри /mnt
дополнительные mounts, и снять их этим же рекурсивным размонтированием.
Это **размонтирование**, не удаление каталогов/данных.
Смена пароля через looom из Live chroot пока не описана: там другие runtime
шаблоны. Для повреждённых credentials, отсутствующего machine profile или
неполного первого bootstrap без соответствующего журнала потребуется отдельный разбор/защищённый backup.
Для прерванного подтверждения обновления аварийной базы актуальный native
`looom bootstrap-recover` согласует Btrfs-профиль и FAT-образ. Подробности:
[bootstrap recovery](runs/2026-10-09-bootstrap-recovery/README.md).

## Что сохранить

Commit SHA, свой YAML/files, base.lock, frozen inputs и package cache.
State backup хранить отдельно и защищённо: там password hashes, Wi-Fi secrets
и SSH keys. Writable bootstrap и его копии тоже содержат секреты. Их не
добавлять в GitHub. Snapshot на том же диске не заменяет backup.

Репозиторий: [Kexfff/looom](https://github.com/Kexfff/looom).
Синтаксис штатных операций: [sgdisk](https://man.archlinux.org/man/sgdisk.8),
[mkfs.btrfs](https://btrfs.readthedocs.io/en/latest/mkfs.btrfs.html),
[pacstrap](https://man.archlinux.org/man/pacstrap.8),
[genfstab](https://man.archlinux.org/man/genfstab.8),
[arch-chroot](https://man.archlinux.org/man/arch-chroot.8) и
[grub-install](https://man.archlinux.org/man/grub-install.8.en).
