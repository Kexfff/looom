# Первый пилот на реальном компьютере

Для установки **с нуля на целый диск**, включая Arch ISO, GPT, Btrfs, обычную
базу, клонирование GitHub, Rust build и KDE, использовать
[полную инструкцию для Intel N100](installation.md).
Ниже — краткий вариант подключения **уже подготовленного** Arch.

Это будущая проверка пользователем. На реальном оборудовании код ещё не запускался.
Поддерживаемый путь сейчас — обычный подготовленный Arch, затем нативное подключение
looom. Автоматического установщика с разметкой диска/ISO пока нет.
[Текущий контракт и команды](native-rust.md).

## Подготовка

Цель: x86_64, UEFI, Secure Boot выключен, диск без шифрования, GRUB + UKI.
Сначала установить и проверить обычный Arch с рабочей сетью и личным пользователем.
Точный набор GPU/Wi-Fi/firmware следует выбрать под компьютер перед пилотом;
Plasma с Mesa проверена пока только на VirtIO. Драйверы NVIDIA и специфическое
оборудование не проверялись. `intel-ucode`/`amd-ucode` при необходимости можно
объявить в packages; произвольные параметры ядра пока отсутствуют в схеме.

Нужны отдельные верхнеуровневые подтомы одного Btrfs и FAT ESP:

| Ресурс | Монтирование | Условие |
| --- | --- | --- |
| `@bootstrap` | `/` | writable обычная система |
| `@home` | `/home` | rw, пользовательские данные |
| `@var` | `/var` | rw, служебное состояние/кэш |
| `@state` | `/var/lib/looom` | rw, root:root 0700 |
| FAT ESP, отдельный раздел | `/efi` | rw, umask=0077, рекомендованы 2 GiB |

Имена подтомов могут отличаться: определяются с установленной машины.
Вложенных друг в друга постоянных подтомов и дополнительных ресурсов в fstab
пилот пока не поддерживает. Существующие дополнительные диски, swap-разделы
или сетевые монтирования нельзя молча терять: bootstrap отклоняет такие записи
до подключения. Их поддержка потребует отдельного обработчика fstab.

Общий `/var` монтируется уже при установке обычного Arch. Если он добавляется
позже, перенос его данных и базы pacman должен быть выполнен до подключения
looom: нельзя скрывать текущую базу пакетов новым пустым монтированием.
Релиз looom затем создаёт собственную базу `/usr/lib/looom/pacman` автоматически.

В исходном Arch нужны рабочие штатные инструменты:

```bash
sudo pacman -S --needed linux linux-firmware btrfs-progs dosfstools grub efibootmgr \
  mkinitcpio systemd-ukify arch-install-scripts openssh sudo networkmanager curl
sudo install -d -m 0700 /var/lib/looom
sudo install -d -m 0700 /etc/NetworkManager/system-connections /etc/looom-local
```

Этот список используется после подготовки диска и монтирований; инструкция
не форматирует разделы. Для kernel.package=linux-lts вместо linux нужен linux-lts.
Личный пользователь должен иметь ожидаемые UID/GID, группу wheel, `/bin/bash`.
У root и пользователя должны быть установлены поддерживаемые password hashes;
первый импорт locked/empty root credential сейчас отклоняется. Задать пароли
обычным `passwd` в подготовленном Arch до подключения looom.

## Подключение

Использовать исходники/бинарник проекта, без копирования состояния VM.
`@bootstrap`, `@toolchain`, credentials, SSH private keys и образы VM содержат
приватные данные и не являются установочным комплектом.
Rust release-бинарник можно собрать в тестовой VM командой
`cargo build --locked --release --bin looom`; runtime требует libxcrypt и
штатные Arch-инструменты. Переносить только этот бинарник и собственную декларацию.

```bash
sudo install -m 0755 looom /usr/bin/looom
sudo /usr/bin/looom --version
```

Создать свой каталог декларации на сохраняемом диске, например
`/var/lib/looom/config/desktop`, на основе `configs/native/desktop`.
Заменить hostname, timezone, имя пользователя, UID/GID и
`password_secret: login-<имя>`. Сам пароль/хэш в YAML не писать.
`login-root`, wheel, shell и два persistent-каталога оставить по контракту.
Дата snapshot — явно выбранная дата архива, не rolling latest.
При ином бинарнике существующий пример base.lock нужно получить заново.

```bash
sudo looom check /var/lib/looom/config/desktop/base.yaml
sudo looom bootstrap /var/lib/looom/config/desktop/base.yaml
sudo looom boot-entry
sudo looom lock /var/lib/looom/config/desktop/base.yaml
sudo looom plan /var/lib/looom/config/desktop/base.yaml
sudo looom build /var/lib/looom/config/desktop/base.yaml pc-1
sudo looom publish pc-1
sudo looom try pc-1
sudo systemctl reboot
```

`bootstrap` сохраняет первоначальный fstab, переносит сохраняемые настройки,
создаёт аварийный UKI и GRUB; повторное подключение защищено от перезаписи.
Частично выполненный bootstrap пока требует явного разбора состояния, а не
удаления каталогов и слепого повтора. `boot-entry` меняет UEFI BootOrder,
создавая looom; прежний fallback loader сохраняется. При отсутствии поддержки
записи NVRAM потребуется выбор загрузчика средствами конкретной прошивки.

## После пробной загрузки

```bash
sudo looom verify
sudo looom status
```

Проверить вход в Plasma, сеть, звук, GPU, Wi-Fi/Bluetooth, сон/пробуждение и
монтирования постоянных данных. Только затем `sudo looom confirm`.
До подтверждения следующий обычный старт вернётся к saved-релизу/bootstrap.
Если сеть недоступна, использовать локальную консоль и меню прошивки/GRUB.

В дальнейшем создать второй релиз с небольшим изменением декларации,
загрузить и подтвердить его, проверить `rollback pc-1` и сохранение `/home`,
локального маркера и нового пароля. Смену пароля делать через
`sudo looom password <имя>`. После проверки снова выбрать рабочий релиз.

Постоянное состояние и его backups требуют отдельной политики резервирования.
Откат root не откатывает данные служб. Secure Boot, шифрование, пользовательский
PAM password change, произвольный fstab и автоматический install ISO — последующие этапы.
