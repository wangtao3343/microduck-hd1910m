# imu_to_dxl 小板：烧录与串口日志

板子是 **STM32G031F8P6 + LSM6DSV16X**，挂在舵机总线上冒充一颗 ID 200 的舵机。

本文件只讲**怎么把固件烧进去、怎么看串口日志**。固件本身在 [`firmware/`](firmware/)，
完整说明见 [`firmware/README.md`](firmware/README.md)。

要烧的文件是仓库里唯一保留的预编译产物
[`firmware/Build/imu_to_dxl.hex`](firmware/Build/imu_to_dxl.hex)。
烧录走 **SWD**（J-Link / ST-Link），不是串口下载。

## 1. 接线（J3）

SWD 只用 **1、2、3、6** 这四脚：

| J3 脚 | 信号 | 接到 |
|---|---|---|
| 1 | GND | 调试器 GND，共地 |
| 2 | SWDCLK | 调试器 SWCLK |
| 3 | SWDIO | 调试器 SWDIO |
| 6 | 3.3V | 调试器 VTref（电压参考） |

- ⚠️ **别把四针排线整排插到 1–4 脚**：第 4 脚是串口输入（UART_RX），不能当供电或参考。
- ⚠️ 第 6 脚只是电压参考，别拿它给板子供电，更别接 5V。

## 2. 烧录

### Linux · J-Link

先装 SEGGER 的 J-Link 软件包（官网下载 Linux 版；装完就有 `JLinkExe` 和 `libjlinkarm.so`）。

```bash
cd firmware
JLinkExe -device STM32G031F8 -if SWD -speed 100 -autoconnect 1
```

进去是 `J-Link>` 提示符，逐条敲：

```
loadfile Build/imu_to_dxl.hex
r
g
exit
```

- `-speed 100` 是 100 kHz。首板在 4 MHz 下载时出过数据传输失败，**初次连接先用这个**，
  连稳了再提速。
- `-device` 的名字得跟 SEGGER 的设备列表对上（本板 STM32G031F8P6），写错会直接报不支持。

### Linux · pyOCD

跟 Mac 上同一条命令。装了 SEGGER 软件包它就能认 J-Link；用 ST-Link 则不用装。

```bash
python3 -m venv .venv
. .venv/bin/activate
pip install pyocd
pyocd flash -t stm32g031f8px Build/imu_to_dxl.hex
```

Linux 上普通用户访问调试器需要 udev 规则，没配就加 `sudo`。

### macOS

```bash
cd firmware
.venv/bin/pyocd flash -t stm32g031f8px Build/imu_to_dxl.hex
```

### Windows

Keil 里按 **F8** 直接下载（工程在 `firmware/MDK-ARM/`），或同上用 pyOCD。

### 烧成功的样子

```
0005078 I Loading .../Build/imu_to_dxl.hex [load_cmd]
0005144 I Erasing... [loader]        [====================] 100%
0005672 I Programming... [loader]    [====================] 100%
0006326 I Erased 12288 bytes (6 sectors), programmed 11264 bytes (11 pages) at 9.32 kB/s
```

## 3. 看串口日志

接 J3 的 **4、5** 脚，**115200 8N1**：

| J3 脚 | 信号 | 接到 USB-TTL |
|---|---|---|
| 1 | GND | GND，共地 |
| 4 | UART_RX | 转接板的 TX |
| 5 | UART_TX | 转接板的 RX |

- ⚠️ **3.3V 电平，别用 5V。** 第 6 脚是 3.3V 参考，不要拿它供电。

每秒一行，长这样：

```
t=70000 ready=1 id=0x70 err=0 n=8271 gyro=-10,4,2 q16=3842,B910,35CD spierr=0 fifo=0 uart=0 crc=0
```

键盘命令：`s` 打状态，`l` 开关周期日志，`r` 重新初始化 IMU，`?` 帮助。

## 4. 这个固件能在 Linux 上编译吗？

### 仓库里这套：不能

工程只有 Keil MDK 一条路（`firmware/MDK-ARM/*.uvprojx` 和 `firmware/Tools/build.ps1`），
调的是 `armclang` / `armasm` / `armlink` / `fromelf` —— Arm Compiler 6 的 **Windows 版**，
Keil 没有 Linux 版。而且启动文件 `firmware/Startup/startup_stm32g031xx.s` 是 **armasm 语法**、
链接脚本 `firmware/MDK-ARM/imu_to_dxl.sct` 是 **Keil 分散加载文件**，GCC 两个都不认。

### 换成 GCC：可以，但要自己补三样

源码本身是能移植的：不依赖 HAL、没有动态内存、没有 RTOS，只用 CMSIS 的寄存器定义和 ST 的
IMU 驱动，全是编译器无关的 C。

1. **GCC 语法的启动文件** —— ST 的 CMSIS 器件包里有 `startup_stm32g031xx.s` 的 GCC 版本，
   或者把仓库里那份 armasm 的转写一遍。
2. **GCC 链接脚本 `.ld`** —— 照 `.sct` 的内存布局写：
   Flash `0x08000000` 共 64 KiB；RAM `0x20000000` 共 6 KiB；栈 `0x20001800` 共 2 KiB。
3. **Makefile** —— 源文件清单、include 路径、宏都能直接抄 `firmware/Tools/build.ps1`：
   5 个 `Core/Src/*.c` + `Drivers/LSM6DSV16X/lsm6dsv16x_reg.c` +
   `Drivers/CMSIS/Device/ST/STM32G0xx/Source/system_stm32g0xx.c` + 启动文件；
   宏是 `STM32G031xx`、`HSE_VALUE=16000000`。目标 Cortex-M0+、`-mthumb`、软浮点。

装上编译器就能编（Ubuntu：`sudo apt install gcc-arm-none-eabi`）。

> **自己编出来的 hex 不能直接替掉原来那份。** servo-web 的 SWD 观察器把固件的 SHA-256 和
> 两个 RAM 符号地址（`sample=0x2000006C`、`tick=0x20000A34`）写死在校验里，换个编译器这些
> 几乎肯定会变。要用 SWD 读法就得重新核对、改代码；走总线（`--imu-bus`）或只看串口日志不受影响。
> 细节见 [`firmware/README.md`](firmware/README.md) 开头。

（这套 GCC 路线仓库里没有现成的，尚未验证过。）
