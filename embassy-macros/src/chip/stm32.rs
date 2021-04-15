use crate::path::ModulePrefix;
use darling::FromMeta;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::spanned::Spanned;

#[derive(Debug, FromMeta, Default)]
pub struct Args {
    #[darling(default)]
    pub embassy_prefix: ModulePrefix,
    #[darling(default)]
    pub use_hse: Option<u32>,
    #[darling(default)]
    pub sysclk: Option<u32>,
    #[darling(default)]
    pub pclk1: Option<u32>,
    #[darling(default)]
    pub require_pll48clk: bool,
}

pub fn generate(args: &Args) -> TokenStream {
    let embassy_path = args.embassy_prefix.append("embassy").path();
    let embassy_stm32_path = args.embassy_prefix.append("embassy_stm32").path();

    let mut clock_cfg_args = quote! {};
    if args.use_hse.is_some() {
        let mhz = args.use_hse.unwrap();
        clock_cfg_args = quote! { #clock_cfg_args.use_hse(#mhz.mhz()) };
    }

    if args.sysclk.is_some() {
        let mhz = args.sysclk.unwrap();
        clock_cfg_args = quote! { #clock_cfg_args.sysclk(#mhz.mhz()) };
    }

    if args.pclk1.is_some() {
        let mhz = args.pclk1.unwrap();
        clock_cfg_args = quote! { #clock_cfg_args.pclk1(#mhz.mhz()) };
    }

    if args.require_pll48clk {
        clock_cfg_args = quote! { #clock_cfg_args.require_pll48clk() };
    }

    quote!(
        use #embassy_stm32_path::{rtc, interrupt, Peripherals, pac, hal, hal::rcc::{Config, RccExt}, hal::time::U32Ext};

        let dp = pac::Peripherals::take().unwrap();
        /*
        #[cfg(not(feature = "chip+stm32l0x2", feature = "stm32l0x2"))]
        let clocks = {
            let rcc = dp.RCC.constrain();
            rcc.cfgr#clock_cfg_args.freeze()
        };*/

        //#[cfg(any(feature = "chip+stm32l0x2", feature = "stm32l0x2"))]
        let mut rcc = dp.RCC.freeze(Config::hsi16());
        let clocks = rcc.clocks;

        unsafe { Peripherals::set_peripherals(clocks) };

        let mut rtc = rtc::RTC::new(dp.TIM2, interrupt::take!(TIM2), clocks);
        let rtc = unsafe { make_static(&mut rtc) };
        rtc.start();
        let mut alarm = rtc.alarm1();

        unsafe { #embassy_path::time::set_clock(rtc) };

        let alarm = unsafe { make_static(&mut alarm) };
        executor.set_alarm(alarm);
    )
}
