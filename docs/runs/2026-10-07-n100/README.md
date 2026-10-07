# Физический пилот Intel N100

Установка 2026-10-07 по SSH. Пользователь явно разрешил полностью стереть старый
NixOS на внутреннем SSD тестового компьютера. Локальный компьютер используется
только для редактирования, SSH и переноса файлов; сборки и проверки выполняются
на VM или на этом разрешённом физическом пилоте.

## Оборудование и область установки

- MINI S, Intel N100, 4 ядра, 16 GiB RAM.
- Внутренний NVMe `/dev/nvme0n1`, модель `512GB SSD`, 476.9 GiB.
- USB `/dev/sda`, OnlyDisk 58 GiB, Ventoy/Arch ISO: не форматируется.
- Intel UHD Graphics `8086:46d1`, драйвер i915.
- Ethernet Realtek `10ec:8168`, драйвер r8169.
- Wi-Fi Intel AX101/CNVi `8086:54f0`, драйвер iwlwifi.
- Intel HDA `8086:54c8`, драйвер snd_hda_intel в Live ISO.
- UEFI, Secure Boot выключен; без шифрования и отдельного swap.

## Воспроизводимость

База исходников: GitHub `Kexfff/looom`, commit
`8d6fa4ca20fd7ee5d151fe43d43657b539837c14`. Архив Arch: `2026/10/03`.
Первоначальный native Rust бинарник собран и проверен ранее в VM;
SHA256 `ad1b202065e3c46559bf917d02480572a612e972616c3d049f1278634512b467`.
Этот бинарник запустил подключение bootstrap. Собственная сборка на N100 дала
тот же SHA256. После аппаратной находки исправлен Rust builder: masked units
получают также drop-in, который mkinitcpio переносит в initramfs. После полного
цикла VM-проверок исправленный менеджер заново собран на N100; его SHA256
`c7007c2fbceebcb87daa35a40e57ff261a5565c8ce195999394166e14c9f31ec`.
Locks A/B получены этим бинарником; исходники рецепта сохранены внутри релизов.

Точный сценарий первоначальной установки: [prepare-arch.sh](prepare-arch.sh).
Он имеет проверки конкретного пилота, Live ISO, UEFI, NVMe, отсутствия mounts и
swap; запускается только с `--erase-authorized-n100`. Это журнал одного
разрешённого запуска, а не универсальный установщик.

1. Проверены оборудование, UUID платы, серийный номер NVMe, mounts, swap,
   Secure Boot, доступность архива и время Live ISO.
2. Создан отдельный ключ управления на клиенте, public key установлен в Live
   root. Приватный ключ остаётся в игнорируемой `.local/physical-access`.
3. SSD заменён на GPT: ESP FAT32 2 GiB + Btrfs на оставшееся место.
4. Созданы `@bootstrap`, `@home`, `@var`, `@state`; все shared mounts подключены
   **до pacstrap**. State root:root 0700, ESP umask=0077.
5. Установлен подписанный Arch из фиксированного архива, включая Rust и
   инструменты сборки только в writable bootstrap.
6. Личный пользователь `owner`, UID/GID 1000, wheel, bash. Начальный пароль
   берётся из уже предоставленной root credential внутри ПК; hash не выводится
   и временный файл удаляется. YAML и журнал не содержат паролей/hash.
7. Ethernet получает временную статическую настройку текущей сети:
   `192.168.1.200/24`, gateway/DNS `192.168.1.1`, интерфейс `enp1s0`.
   Это сохраняет управление при смене Live ISO на NetworkManager. В другой
   сети нужно изменить persistent NM profile; адрес не является универсальным.
8. Сохранены известные клиенту SSH host keys; ключ управления установлен root
   и owner. SSH допускает только ключи. Ключи/credentials не экспортируются в Git.
9. База pacman переносится в `/usr/lib/looom/pacman`, затем подключаются native
   bootstrap UKI, GRUB и запись UEFI. Обычная промежуточная установка GRUB из
   общей инструкции пропущена: native bootstrap проверен в chroot в VM и
   напрямую создаёт загрузку `@bootstrap`.

## План приёмки

- Первая настоящая загрузка writable bootstrap, сохранение SSH и сети.
- Native Rust сборка на целевом ПК; package locks привязаны к её SHA.
- `n100-a`: Plasma, linux, Intel microcode, SSH, декларативный marker A.
- `n100-b`: Plasma, linux-lts, пакет tree, marker B, masked bluetooth.service.
- Одноразовая загрузка и подтверждение A/B, затем откат B → A и возврат B.
- Read-only Btrfs root, отдельная база пакетов, shared home/var/state и только
  разрешённые local /etc; стабильные UID/GID, credentials и SSH identity.
- PAM login/SDDM/sudo, фактическая сессия Plasma Wayland и аудиослужбы.
- Отказы записи в root и stale/corrupted inputs, безопасный preview GC.

Загрузка архива шла медленно. Pacman был прерван SIGINT на этапе скачивания;
кэш `/var/cache/pacman/pkg` передан из ранее проверенной VM по SSH во временный
каталог на SSD, затем перемещены package archives и signatures. Pacstrap
возобновлён с `--resume-packages`, который проверяет готовые mounts и **не**
форматирует диск. Сохраняется фиксированный архив и обязательная проверка подписей.
При первом chroot возник `Device or resource busy` при замене resolv.conf:
arch-chroot bind-mounts этот файл. Замена перенесена после всех chroot вызовов,
создание пользователя при повторе сделано идемпотентным. Повторное подключение
выполнялось до переноса базы pacman и регистрации looom.

В VM выполнены cargo fmt, clippy всех targets с `-D warnings`, семь тестов
(5 config + bootstrap fixture + runtime suite), release build обоих помощников.
После адаптации пути конфигурации повторены clippy/build и отрицательные inputs;
физический помощник отвергает VM. Исходный VM-релиз повторно прошёл `looom verify`.

Rust entry point `looom-n100-test` отдельно проверяет identity разрешённого
пилота. VM entry point сохраняет исходные QEMU/KVM + MAC guards. Разрушительные
runtime fixtures остаются только в VM; приёмочные помощники не входят в релизы.

## Результаты

| Проверка | Фактический результат |
| --- | --- |
| Загрузка SSD | Writable `@bootstrap`, SSH host identity и адрес сохранены |
| A | `n100-a`, linux `7.2.8-arch1-2`, 652 пакета, read-only `@root-n100-a`, confirmed |
| B | `n100-b`, linux-lts `6.18.54-2-lts`, 653 пакета, read-only `@root-n100-b`, confirmed |
| Неподтверждённый trial | A → следующий reboot вернул bootstrap; saved_entry не менялся |
| Откат B → A | Вернулись ядро, marker A и отсутствие tree/bluetooth mask |
| Возврат A → B | Вернулись LTS, tree, marker B; B выбран постоянным |
| Общие данные | Home marker, local /etc marker, UID/GID и пароль сохранились |
| PAM | login/SDDM/sudo принимают правильный и отвергают неправильный пароль до и после отката |
| Реальный GUI | Вход через target uinput, `owner`, seat0, активная Plasma Wayland на обоих ядрах |
| GPU | KWin OpenGL/EGL: Intel, Mesa Intel(R) Graphics (ADL-N), OpenGL 4.6 |
| Audio | HDA PCH определяется; PipeWire, Pulse и WirePlumber active |
| Read-only | Запись в /etc и прямой pacman -S tree отказали с EROFS; файл/пакет не появились |
| Credentials | owner не может читать private hash; runtime/persistent значения совпадают |
| Ошибочные действия | confirm чужого running release и rollback неподтверждённого отвергнуты без изменения grubenv |
| Входы сборки | Stale recipe и corrupt archive SHA отвергнуты до создания frozen inputs |
| Сон | Один S3/deep цикл на LTS; RTC на 20 секунд; сеть, GUI и verify после resume успешны |
| Журналы | Проверены на точные текущие hashes и временный password; намеренная private fixture обнаружена и удалена до экспорта |
| Очистка | Исходный пароль возвращён, private-password-test удалён, GC только preview, A/B сохранены |

Проверки подтверждаются файлами [evidence](evidence/). Оригинальные данные
сборок, конфигурации, locks и журналы также находятся на ПК в `/var/lib/looom`.
`native-final-state.json` фиксирует итоговые метаданные, digest менеджера,
инвентарь и hashes встроенных исходников без credentials.

### Аварийный bootstrap и TPM

Первый bootstrap сохранил раннюю TPM NvPCR-ошибку, хотя SSH-восстановление
работало. Затем исправленный read-only профиль запретил службы
`systemd-tpm2-setup-early.service`, `systemd-pcrproduct.service` и
`systemd-pcrlogin@.service`. Только маски основного root недостаточно:
mkinitcpio копирует vendor unit и drop-ins, но не /etc symlink mask.
Rust builder добавляет всегда ложный `ConditionPathExists=/dev/null/looom-masked`.
Наличие drop-in проверено внутри настоящего UKI; A/B загрузились без failed units.

Старый аварийный образ отдельно обновлён ограниченной native Rust процедурой
`looom-n100-test refresh-bootstrap`. Она требует именно этот разрешённый ПК,
подтверждённый здоровый running release и совпадающий manager в @bootstrap;
удерживает общий lock, сохраняет исходный UKI и machine profile в закрытом
`/var/lib/looom/dev/bootstrap-refresh`, строит новый образ, проверяет его состав
и digest, записывает prepare/commit journal и обновляет доверенный digest.
Постоянный выбор B не меняется. Это процедура ремонта данного пилота,
не общий публичный bootstrap-upgrade API.

Старый UKI SHA256: `94d301ede69f0ec2dd911e75358e8a10a8ebb2de474c2ab395e442575127e8cd`.
Новый UKI SHA256: `83dda4a4fe0e1633d72ecd226cf06415cc49a3ad60f9510bfbc142634e61da0c`.
Через grubenv выбран **один** boot bootstrap при сохранённом B. Реальный
bootstrap загрузился без failed units, с активными SSH/NM/timesync и пропущенной
TPM-службой по условию. Следующий reboot снова вернул сохранённый B.

У замены UKI и профиля остаётся окно power failure между двумя файлами;
это отдельно от атомарной сборки релизов. Для разбора сохранены `original.efi`,
`original-machine.json`, `updated-machine.json` и `journal.json` в root-only
каталоге. Сопоставить фактический SHA EFI-образа с old/new SHA из journal и
согласовать соответствующий профиль через временный файл, fsync и rename.
Подтверждённые A/B и saved_entry ремонта не затрагиваются. Power-cut этой
процедуры не проверен; автоматического восстановления первого bootstrap и
его обновления в MVP пока нет.

### Что осталось за пределами приёмки

Не проверены подключение Wi-Fi к точке доступа, Bluetooth, звук через реальные
колонки/наушники и микрофон; обнаружение устройств и active audio services
не доказывают физическое воспроизведение. Сон проверен один раз только на LTS,
через RTC, а не кнопкой/крышкой и не долгими многократными циклами. Нет испытаний
power-cut FAT, полного Btrfs ENOSPC, смены всей даты архива, Secure Boot,
шифрования и другого оборудования. Профиль B — тестовая база с KDE и tree;
пользовательские приложения и универсальный установщик — следующие этапы.

### Доступ и изменения

С клиентского компьютера, где выполнялась установка:

```bash
./scripts/physical-ssh.sh
# В открывшейся root-сессии:
looom status
looom verify
looom password owner
```

Или интерактивная смена пароля одной командой:

```bash
./scripts/physical-ssh.sh --tty 'looom password owner'
```

Ключ управления остаётся в клиентской `.local/physical-access` и не включён в
Git. SSH допускает только ключи. Локальный пользователь — `owner`, начальный
пароль тот же, который был предоставлен для Live SSH; временные приёмочные
пароли удалены. Для доступа с другого клиента добавить его **public** key в
`/home/owner/.ssh/authorized_keys` или persistent root-ssh, приватный не копировать
в конфигурацию или репозиторий. Sudo требует пароль.

Рабочие декларации: `/var/lib/looom/config/desktop-a/base.yaml` и `desktop-b/base.yaml`.
Для следующего изменения выбрать новый release ID и выполнить lock/plan/build,
publish/try/reboot, verify/confirm. В действующем read-only root pacman напрямую
не устанавливает пакеты. SSH нужен до подтверждения каждой новой загрузки.
