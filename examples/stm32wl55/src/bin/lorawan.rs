#![no_std]
#![no_main]
#![macro_use]
#![allow(dead_code)]
#![feature(generic_associated_types)]
#![feature(type_alias_impl_trait)]

#[path = "../example_common.rs"]
mod example_common;

use embassy_lora::{stm32wl::*, LoraTimer};
use embassy_stm32::{
    dbgmcu::Dbgmcu,
    gpio::{Level, Output, Pin, Speed},
    interrupt, pac, rcc,
    rng::Rng,
    subghz::*,
    Peripherals,
};
use lorawan_device::async_device::{region, Device, JoinMode};
use lorawan_encoding::default_crypto::DefaultFactory as Crypto;

fn config() -> embassy_stm32::Config {
    let mut config = embassy_stm32::Config::default();
    config.rcc = config.rcc.clock_src(embassy_stm32::rcc::ClockSrc::HSI16);
    config
}

#[embassy::main(config = "config()")]
async fn main(_spawner: embassy::executor::Spawner, p: Peripherals) {
    unsafe {
        Dbgmcu::enable_all();
        let mut rcc = rcc::Rcc::new(p.RCC);
        rcc.enable_lsi();
        pac::RCC.ccipr().modify(|w| {
            w.set_rngsel(0b01);
        });
    }

    let ctrl1 = Output::new(p.PC3.degrade(), Level::High, Speed::High);
    let ctrl2 = Output::new(p.PC4.degrade(), Level::High, Speed::High);
    let ctrl3 = Output::new(p.PC5.degrade(), Level::High, Speed::High);
    let rfs = RadioSwitch::new(ctrl1, ctrl2, ctrl3);

    let irq = interrupt::take!(SUBGHZ_RADIO);
    let radio = SubGhz::new(
        p.SUBGHZSPI,
        p.PA5,
        p.PA7,
        p.PA6,
        p.DMA1_CH0,
        p.DMA1_CH1,
        irq,
    );
    let radio = SubGhzRadio::new(radio, rfs);

    let region = region::EU868::default().into();
    let mut radio_buffer = [0; 256];
    let mut device: Device<'_, _, Crypto, _, _> = Device::new(
        region,
        radio,
        LoraTimer,
        Rng::new(p.RNG),
        &mut radio_buffer[..],
    );

    defmt::info!("Joining LoRaWAN network");

    // TODO: Adjust the EUI and Keys according to your network credentials
    device
        .join(&JoinMode::OTAA {
            deveui: [0x70, 0xB3, 0xD5, 0x7E, 0xD0, 0x04, 0x53, 0xC1],
            appeui: [0x70, 0xB3, 0xD5, 0x7E, 0xD0, 0x03, 0xD6, 0xEC],
            appkey: [
                0x8F, 0x31, 0x78, 0x0D, 0xF3, 0x48, 0xFD, 0x79, 0x82, 0x8A, 0xD1, 0x0D, 0x7F, 0x08,
                0x2D, 0x4B,
            ],
        })
        .await
        .ok()
        .unwrap();
    defmt::info!("LoRaWAN network joined");

    defmt::info!("Sending 'PING'");
    device.send(b"PING", 1, false).await.ok().unwrap();
    defmt::info!("Message sent!");
}
