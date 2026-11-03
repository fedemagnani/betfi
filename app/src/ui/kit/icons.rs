//! Icons. The mockup draws Google's Material Symbols Outlined (weight 400); the kit carries the
//! ones it uses as inline SVG paths ([`Sym`], 960-unit grid, filled in `currentColor`), copied
//! from `@material-symbols/svg-400@0.47.6` (Apache License 2.0, Copyright Google LLC), so no icon
//! font is fetched. Decorative: every icon is `aria-hidden`; the control around it carries the
//! accessible name.
//!
//! The L6 names ([`Icon`], [`Svg`]) still work for the screens not yet redesigned; they now draw
//! the Material shapes too.

use dioxus::prelude::*;

/// A Material Symbol by its mockup name: `<span class="material-symbols-outlined">bolt</span>`
/// becomes `SymIcon { sym: Sym::Receipt, size: 22 }`. `…Fill` is the filled variant (the mockup's
/// `font-variation-settings: 'FILL' 1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sym {
    /// `add`: + (DEPOSIT).
    Add,
    /// `person`: avatar placeholder.
    Person,
    /// `person_add`: Follow.
    PersonAdd,
    /// `sports_basketball`: Basketball tab, leg sport.
    Basketball,
    /// `sports_soccer`: Soccer tab, Sports nav tab.
    Soccer,
    /// `sports_tennis`: not offered (V-2).
    Tennis,
    /// `group`: Tipsters nav tab, copiers.
    Group,
    /// `confirmation_number`: the bets' Open tab.
    Ticket,
    /// `account_balance_wallet`: Account nav tab.
    Wallet,
    /// `account_balance_wallet-fill`: balance chip.
    WalletFill,
    /// `receipt_long`: slip pill, Schedina sheet, receipt.
    Receipt,
    /// `close`: sheet close, remove a pick.
    Close,
    /// `keyboard_arrow_down`: currency pill.
    ExpandMore,
    /// `keyboard_arrow_up`: collapse.
    ExpandLess,
    /// `arrow_forward`: links and CTAs.
    ArrowForward,
    /// `arrow_back`: back.
    ArrowBack,
    /// `chevron_right`: SCHEDINA chevron.
    ChevronRight,
    /// `open_in_new`: Solscan.
    OpenInNew,
    /// `arrow_outward`: outward arrow.
    ArrowOutward,
    /// `verified`: Solscan chip.
    Verified,
    /// `verified-fill`: verified badge, tx confirmed.
    VerifiedFill,
    /// `schedule`: Upcoming matches.
    Schedule,
    /// `content_copy`: Copy Schedina CTA.
    ContentCopy,
    /// `share`: share.
    Share,
    /// `check`: cash-out tick.
    Check,
    /// `check_circle`: selected outcome.
    CheckCircle,
    /// `check_circle-fill`: done.
    CheckCircleFill,
    /// `token`: SOL chip.
    Token,
    /// `token-fill`: SOL chip, filled.
    TokenFill,
    /// `timer`: live clock.
    Timer,
    /// `touch_app`: tap to pick.
    TouchApp,
    /// `pie_chart`: Staked Sentiment.
    PieChart,
    /// `lock`: settled.
    Lock,
    /// `lock_clock`: quote countdown.
    LockClock,
    /// `swap_horiz`: Match Winner.
    SwapHoriz,
    /// `post_add`: Select to bet.
    PostAdd,
    /// `star_shine`: celebration sparkle.
    StarShine,
    /// `live_tv`: YouTube link.
    LiveTv,
    /// `send`: Telegram link.
    Send,
    /// `photo_camera`: Instagram link.
    PhotoCamera,
    /// `travel_explore`: explorer.
    TravelExplore,
    /// `autorenew`: price moved.
    Refresh,
    /// `home`: home.
    Home,
}

impl Sym {
    pub const ALL: [Sym; 43] = [
        Sym::Add,
        Sym::Person,
        Sym::PersonAdd,
        Sym::Basketball,
        Sym::Soccer,
        Sym::Tennis,
        Sym::Group,
        Sym::Ticket,
        Sym::Wallet,
        Sym::WalletFill,
        Sym::Receipt,
        Sym::Close,
        Sym::ExpandMore,
        Sym::ExpandLess,
        Sym::ArrowForward,
        Sym::ArrowBack,
        Sym::ChevronRight,
        Sym::OpenInNew,
        Sym::ArrowOutward,
        Sym::Verified,
        Sym::VerifiedFill,
        Sym::Schedule,
        Sym::ContentCopy,
        Sym::Share,
        Sym::Check,
        Sym::CheckCircle,
        Sym::CheckCircleFill,
        Sym::Token,
        Sym::TokenFill,
        Sym::Timer,
        Sym::TouchApp,
        Sym::PieChart,
        Sym::Lock,
        Sym::LockClock,
        Sym::SwapHoriz,
        Sym::PostAdd,
        Sym::StarShine,
        Sym::LiveTv,
        Sym::Send,
        Sym::PhotoCamera,
        Sym::TravelExplore,
        Sym::Refresh,
        Sym::Home,
    ];

    /// The Material Symbols name (the mockup's ligature text).
    pub fn name(self) -> &'static str {
        match self {
            Sym::Add => "add",
            Sym::Person => "person",
            Sym::PersonAdd => "person_add",
            Sym::Basketball => "sports_basketball",
            Sym::Soccer => "sports_soccer",
            Sym::Tennis => "sports_tennis",
            Sym::Group => "group",
            Sym::Ticket => "confirmation_number",
            Sym::Wallet => "account_balance_wallet",
            Sym::WalletFill => "account_balance_wallet-fill",
            Sym::Receipt => "receipt_long",
            Sym::Close => "close",
            Sym::ExpandMore => "keyboard_arrow_down",
            Sym::ExpandLess => "keyboard_arrow_up",
            Sym::ArrowForward => "arrow_forward",
            Sym::ArrowBack => "arrow_back",
            Sym::ChevronRight => "chevron_right",
            Sym::OpenInNew => "open_in_new",
            Sym::ArrowOutward => "arrow_outward",
            Sym::Verified => "verified",
            Sym::VerifiedFill => "verified-fill",
            Sym::Schedule => "schedule",
            Sym::ContentCopy => "content_copy",
            Sym::Share => "share",
            Sym::Check => "check",
            Sym::CheckCircle => "check_circle",
            Sym::CheckCircleFill => "check_circle-fill",
            Sym::Token => "token",
            Sym::TokenFill => "token-fill",
            Sym::Timer => "timer",
            Sym::TouchApp => "touch_app",
            Sym::PieChart => "pie_chart",
            Sym::Lock => "lock",
            Sym::LockClock => "lock_clock",
            Sym::SwapHoriz => "swap_horiz",
            Sym::PostAdd => "post_add",
            Sym::StarShine => "star_shine",
            Sym::LiveTv => "live_tv",
            Sym::Send => "send",
            Sym::PhotoCamera => "photo_camera",
            Sym::TravelExplore => "travel_explore",
            Sym::Refresh => "autorenew",
            Sym::Home => "home",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Sym::Add => "M450-450H200v-60h250v-250h60v250h250v60H510v250h-60v-250Z",
            Sym::Person => {
                "M372-523q-42-42-42-108t42-108q42-42 108-42t108 42q42 42 42 108t-42 108q-42 42-108 42t-108-42ZM160-160v-94q0-38 19-65t49-41q67-30 128.5-45T480-420q62 0 123 15.5T731-360q31 14 50 41t19 65v94H160Zm60-60h520v-34q0-16-9.5-30.5T707-306q-64-31-117-42.5T480-360q-57 0-111 11.5T252-306q-14 7-23 21.5t-9 30.5v34Zm324.5-346.5Q570-592 570-631t-25.5-64.5Q519-721 480-721t-64.5 25.5Q390-670 390-631t25.5 64.5Q441-541 480-541t64.5-25.5ZM480-631Zm0 411Z"
            }
            Sym::PersonAdd => {
                "M730-400v-130H600v-60h130v-130h60v130h130v60H790v130h-60ZM252-523q-42-42-42-108t42-108q42-42 108-42t108 42q42 42 42 108t-42 108q-42 42-108 42t-108-42ZM40-160v-94q0-35 17.5-63.5T108-360q75-33 133.5-46.5T360-420q60 0 118 13.5T611-360q33 15 51 43t18 63v94H40Zm60-60h520v-34q0-16-9-30.5T587-306q-71-33-120-43.5T360-360q-58 0-107.5 10.5T132-306q-15 7-23.5 21.5T100-254v34Zm324.5-346.5Q450-592 450-631t-25.5-64.5Q399-721 360-721t-64.5 25.5Q270-670 270-631t25.5 64.5Q321-541 360-541t64.5-25.5ZM360-631Zm0 411Z"
            }
            Sym::Basketball => {
                "M141-510h137q-7-48-28-90.5T198-674q-23 35-39 76t-18 88Zm541 0h137q-2-47-18-88t-39-76q-34 34-53.5 75T682-510ZM198-287q34-34 53.5-74.5T278-450H141q2 47 18 87.5t39 75.5Zm564 0q23-35 39-75.5t18-87.5H682q7 48 26.5 88.5T762-287ZM339-510h111v-309q-62 7-115.5 32T238-720q41 41 67 95t34 115Zm171 0h111q8-61 34.5-115t67.5-95q-43-42-97-67t-116-32v309Zm-60 369v-309H339q-8 61-34 114.5T238-241q43 42 94 67.5T450-141Zm60 0q67-7 118.5-32.5T723-241q-41-41-67.5-94.5T621-450H510v309Zm-30-334Zm0 395q-83 0-156-31.5T197-197q-54-54-85.5-127T80-480q0-83 31.5-156T197-763q54-54 127-85.5T480-880q83 0 156 31.5T763-763q54 54 85.5 127T880-480q0 83-31.5 156T763-197q-54 54-127 85.5T480-80Z"
            }
            Sym::Soccer => {
                "M480-80q-82 0-155-31.5t-127.5-86Q143-252 111.5-325T80-480.5q0-82.5 31.5-155t86-127Q252-817 325-848.5T480-880q83 0 155.5 31.5t127 86q54.5 54.5 86 127t31.5 155q0 82.5-31.5 155.5t-86 127.5q-54.5 54.5-127 86T480-80Zm205-493 69-24 18-64q-34-53-84-90.5T575-809l-65 43v70l175 123Zm-409 0 174-123v-70l-64-43q-63 20-113 57.5T189-661l22 64 65 24Zm-54 320 60-7 39-65-62-191-71-24-48 39q0 72 16 131.5T222-253Zm312.5 108q27.5-5 57.5-13l33-72-32-55H368l-32 55 33 72q26 8 55 13t56 5q27 0 54.5-5ZM374-345h208l61-183-163-117-166 117 60 183Zm365 92q49-57 65-116.5T820-501l-48-33-70 18-62 191 38 65 61 7Z"
            }
            Sym::Tennis => {
                "m123-159-43-43 174-174q33-33 47-82.5T315-597q0-57 25-112t76-106q88-88 193-100t173 56q68 68 56 174T738-491q-51 51-106 75t-112 24q-89 0-139.5 13T297-333L123-159Zm290-332q49 49 133 36.5T697-534q67-67 80-149.5T741-815q-49-49-132.5-36.5T458-772q-67 67-80.5 149.5T413-491ZM634-82q-42-42-42-102t42-102q42-42 102-42t102 42q42 42 42 102T838-82q-42 42-102 42T634-82Zm162-42q24-24 24-60t-24-60q-24-24-60-24t-60 24q-24 24-24 60t24 60q24 24 60 24t60-24Zm-60-60Z"
            }
            Sym::Group => {
                "M38-160v-94q0-35 18-63.5t50-42.5q73-32 131.5-46T358-420q62 0 120 14t131 46q32 14 50.5 42.5T678-254v94H38Zm700 0v-94q0-63-32-103.5T622-423q69 8 130 23.5t99 35.5q33 19 52 47t19 63v94H738ZM250-523q-42-42-42-108t42-108q42-42 108-42t108 42q42 42 42 108t-42 108q-42 42-108 42t-108-42Zm426 0q-42 42-108 42-11 0-24.5-1.5T519-488q24-25 36.5-61.5T568-631q0-45-12.5-79.5T519-774q11-3 24.5-5t24.5-2q66 0 108 42t42 108q0 66-42 108ZM98-220h520v-34q0-16-9.5-31T585-306q-72-32-121-43t-106-11q-57 0-106.5 11T130-306q-14 6-23 21t-9 31v34Zm324.5-346.5Q448-592 448-631t-25.5-64.5Q397-721 358-721t-64.5 25.5Q268-670 268-631t25.5 64.5Q319-541 358-541t64.5-25.5ZM358-220Zm0-411Z"
            }
            Sym::Ticket => {
                "M480-283q12 0 21-9t9-21q0-12-9-21t-21-9q-12 0-21 9t-9 21q0 12 9 21t21 9Zm0-167q12 0 21-9t9-21q0-12-9-21t-21-9q-12 0-21 9t-9 21q0 12 9 21t21 9Zm0-167q12 0 21-9t9-21q0-12-9-21t-21-9q-12 0-21 9t-9 21q0 12 9 21t21 9Zm340 457H140q-24.75 0-42.37-17.63Q80-195.25 80-220v-153q37-8 61.5-37.5T166-480q0-40-24.5-70T80-587v-153q0-24.75 17.63-42.38Q115.25-800 140-800h680q24.75 0 42.38 17.62Q880-764.75 880-740v153q-37 7-61.5 37T794-480q0 40 24.5 69.5T880-373v153q0 24.75-17.62 42.37Q844.75-160 820-160Zm0-60v-109q-38-26-62-65t-24-86q0-47 24-86t62-65v-109H140v109q39 26 62.5 65t23.5 86q0 47-23.5 86T140-329v109h680ZM480-480Z"
            }
            Sym::Wallet => {
                "M180-233v53-600 547Zm0 113q-24.75 0-42.37-17.63Q120-155.25 120-180v-600q0-24.75 17.63-42.38Q155.25-840 180-840h600q24.75 0 42.38 17.62Q840-804.75 840-780v134h-60v-134H180v600h600v-133h60v133q0 24.75-17.62 42.37Q804.75-120 780-120H180Zm358-173q-30.52 0-52.26-21.44Q464-335.89 464-366v-227q0-30.11 21.74-51.56Q507.48-666 538-666h270q30.53 0 52.26 21.44Q882-623.11 882-593v227q0 30.11-21.74 51.56Q838.53-293 808-293H538Zm284-60v-253H524v253h298Zm-124.5-81.96Q716-453.92 716-481q0-26.25-19-44.63Q678-544 652-544t-45 18.37q-19 18.38-19 44.63 0 27.08 18.74 46.04Q625.47-416 652.24-416q26.76 0 45.26-18.96Z"
            }
            Sym::WalletFill => {
                "M180-120q-23 0-41.5-18T120-180v-600q0-23 18.5-41.5T180-840h600q24 0 42 18.5t18 41.5v54H538q-58 0-96 38t-38 95v227q0 57 38 95t96 38h302v53q0 24-18 42t-42 18H180Zm358-173q-34 0-54-20t-20-53v-227q0-34 20-53.5t54-19.5h269q34 0 54 19.5t20 53.5v227q0 33-20 53t-54 20H538Zm158.5-142.5Q716-455 716-481q0-25-19.5-44T652-544q-25 0-44.5 19T588-481q0 26 19.5 45.5T652-416q25 0 44.5-19.5Z"
            }
            Sym::Receipt => {
                "M222-80q-43.75 0-74.37-30.63Q117-141.25 117-185v-125h127v-570l59.8 60 59.8-60 59.8 60 59.8-60 59.8 60 60-60 60 60 60-60 60 60 60-60v695q0 43.75-30.62 74.37Q781.75-80 738-80H222Zm516-60q20 0 32.5-12.5T783-185v-595H304v470h389v125q0 20 12.5 32.5T738-140ZM357-622v-60h240v60H357Zm0 134v-60h240v60H357Zm333-134q-12 0-21-9t-9-21q0-12 9-21t21-9q12 0 21 9t9 21q0 12-9 21t-21 9Zm0 129q-12 0-21-9t-9-21q0-12 9-21t21-9q12 0 21 9t9 21q0 12-9 21t-21 9ZM221-140h412v-110H177v65q0 20 12.65 32.5T221-140Zm-44 0v-110 110Z"
            }
            Sym::Close => {
                "m249-207-42-42 231-231-231-231 42-42 231 231 231-231 42 42-231 231 231 231-42 42-231-231-231 231Z"
            }
            Sym::ExpandMore => "M480-344 240-584l43-43 197 197 197-197 43 43-240 240Z",
            Sym::ExpandLess => "M480-554 283-357l-43-43 240-240 240 240-43 43-197-197Z",
            Sym::ArrowForward => {
                "M686-450H160v-60h526L438-758l42-42 320 320-320 320-42-42 248-248Z"
            }
            Sym::ArrowBack => "m274-450 248 248-42 42-320-320 320-320 42 42-248 248h526v60H274Z",
            Sym::ChevronRight => "M530-481 332-679l43-43 241 241-241 241-43-43 198-198Z",
            Sym::OpenInNew => {
                "M180-120q-24 0-42-18t-18-42v-600q0-24 18-42t42-18h279v60H180v600h600v-279h60v279q0 24-18 42t-42 18H180Zm202-219-42-43 398-398H519v-60h321v321h-60v-218L382-339Z"
            }
            Sym::ArrowOutward => "m242-246-42-42 412-412H234v-60h480v480h-60v-378L242-246Z",
            Sym::Verified => {
                "m346-60-76-130-151-31 17-147-96-112 96-111-17-147 151-31 76-131 134 62 134-62 77 131 150 31-17 147 96 111-96 112 17 147-150 31-77 130-134-62-134 62Zm27-79 107-45 110 45 67-100 117-30-12-119 81-92-81-94 12-119-117-28-69-100-108 45-110-45-67 100-117 28 12 119-81 94 81 92-12 121 117 28 70 100Zm107-341Zm-43 133 227-225-45-41-182 180-95-99-46 45 141 140Z"
            }
            Sym::VerifiedFill => {
                "m346-60-76-130-151-31 17-147-96-112 96-111-17-147 151-31 76-131 134 62 134-62 77 131 150 31-17 147 96 111-96 112 17 147-150 31-77 130-134-62-134 62Zm91-287 227-225-45-41-182 180-95-99-46 45 141 140Z"
            }
            Sym::Schedule => {
                "m627-287 45-45-159-160v-201h-60v225l174 181ZM480-80q-82 0-155-31.5t-127.5-86Q143-252 111.5-325T80-480q0-82 31.5-155t86-127.5Q252-817 325-848.5T480-880q82 0 155 31.5t127.5 86Q817-708 848.5-635T880-480q0 82-31.5 155t-86 127.5Q708-143 635-111.5T480-80Zm0-400Zm0 340q140 0 240-100t100-240q0-140-100-240T480-820q-140 0-240 100T140-480q0 140 100 240t240 100Z"
            }
            Sym::ContentCopy => {
                "M300-200q-24 0-42-18t-18-42v-560q0-24 18-42t42-18h440q24 0 42 18t18 42v560q0 24-18 42t-42 18H300Zm0-60h440v-560H300v560ZM180-80q-24 0-42-18t-18-42v-620h60v620h500v60H180Zm120-180v-560 560Z"
            }
            Sym::Share => {
                "M686-80q-47.5 0-80.75-33.25T572-194q0-8 5-34L278-403q-16.28 17.34-37.64 27.17Q219-366 194-366q-47.5 0-80.75-33T80-480q0-48 33.25-81T194-594q24 0 45 9.3 21 9.29 37 25.7l301-173q-2-8-3.5-16.5T572-766q0-47.5 33.25-80.75T686-880q47.5 0 80.75 33.25T800-766q0 47.5-33.25 80.75T686-652q-23.27 0-43.64-9Q622-670 606-685L302-516q3 8 4.5 17.5t1.5 18q0 8.5-1 16t-3 15.5l303 173q16-15 36.09-23.5 20.1-8.5 43.07-8.5Q734-308 767-274.75T800-194q0 47.5-33.25 80.75T686-80Zm.04-60q22.96 0 38.46-15.54 15.5-15.53 15.5-38.5 0-22.96-15.54-38.46-15.53-15.5-38.5-15.5-22.96 0-38.46 15.54-15.5 15.53-15.5 38.5 0 22.96 15.54 38.46 15.53 15.5 38.5 15.5Zm-492-286q22.96 0 38.46-15.54 15.5-15.53 15.5-38.5 0-22.96-15.54-38.46-15.53-15.5-38.5-15.5-22.96 0-38.46 15.54-15.5 15.53-15.5 38.5 0 22.96 15.54 38.46 15.53 15.5 38.5 15.5ZM724.5-727.54q15.5-15.53 15.5-38.5 0-22.96-15.54-38.46-15.53-15.5-38.5-15.5-22.96 0-38.46 15.54-15.5 15.53-15.5 38.5 0 22.96 15.54 38.46 15.53 15.5 38.5 15.5 22.96 0 38.46-15.54ZM686-194ZM194-480Zm492-286Z"
            }
            Sym::Check => "M378-246 154-470l43-43 181 181 384-384 43 43-427 427Z",
            Sym::CheckCircle => {
                "m421-298 283-283-46-45-237 237-120-120-45 45 165 166Zm59 218q-82 0-155-31.5t-127.5-86Q143-252 111.5-325T80-480q0-83 31.5-156t86-127Q252-817 325-848.5T480-880q83 0 156 31.5T763-763q54 54 85.5 127T880-480q0 82-31.5 155T763-197.5q-54 54.5-127 86T480-80Zm0-60q142 0 241-99.5T820-480q0-142-99-241t-241-99q-141 0-240.5 99T140-480q0 141 99.5 240.5T480-140Zm0-340Z"
            }
            Sym::CheckCircleFill => {
                "m421-298 283-283-46-45-237 237-120-120-45 45 165 166Zm59 218q-82 0-155-31.5t-127.5-86Q143-252 111.5-325T80-480q0-83 31.5-156t86-127Q252-817 325-848.5T480-880q83 0 156 31.5T763-763q54 54 85.5 127T880-480q0 82-31.5 155T763-197.5q-54 54.5-127 86T480-80Z"
            }
            Sym::Token => {
                "M480-80 120-286v-399l360-195 360 195v399L480-80ZM366-578q23-23 52.5-37.5T480-630q32 0 61.5 14.5T594-578l153-90-267-142-267 142 153 90Zm84 412v-167q-52-14-86-54t-34-93q0-11 1.5-22.5T337-525l-157-93v297l270 155Zm94-250q26-26 26-64t-26-64q-26-26-64-26t-64 26q-26 26-26 64t26 64q26 26 64 26t64-26Zm-34 250 270-155v-297l-157 93q4 11 5.5 22.5T630-480q0 53-34 93t-86 54v167Z"
            }
            Sym::TokenFill => {
                "M366-578 153-703l327-177 327 177-213 125q-23-23-52.5-37.5T480-630q-32 0-61.5 14.5T366-578Zm84 481L120-286v-368l217 129q-4 11-5.5 22.5T330-480q0 53 34 93t86 54v236Zm-34-319q-26-26-26-64t26-64q26-26 64-26t64 26q26 26 26 64t-26 64q-26 26-64 26t-64-26Zm94 319v-236q52-14 86-54t34-93q0-11-1.5-22.5T623-525l217-129v368L510-97Z"
            }
            Sym::Timer => {
                "M360-860v-60h240v60H360Zm90 447h60v-230h-60v230ZM340.5-109.5Q275-138 226-187t-77.5-114.5Q120-367 120-441t28.5-139.5Q177-646 226-695t114.5-77.5Q406-801 480-801q67 0 126 22.5T711-716l51-51 42 42-51 51q36 40 61.5 97T840-441q0 74-28.5 139.5T734-187q-49 49-114.5 77.5T480-81q-74 0-139.5-28.5Zm352-119Q780-316 780-441t-87.5-212.5Q605-741 480-741t-212.5 87.5Q180-566 180-441t87.5 212.5Q355-141 480-141t212.5-87.5ZM480-440Z"
            }
            Sym::TouchApp => {
                "M445-80q-29 0-56-12t-45-35L127-403l21-23q14-15 34.5-18.5T221-438l99 53v-365q0-12.75 8.68-21.38 8.67-8.62 21.5-8.62 12.82 0 21.32 8.62 8.5 8.63 8.5 21.38v465l-144-77 156 198q10 12 23.76 18 13.76 6 29.24 6h205q38 0 64-26t26-64v-170q0-25.5-17.25-42.75T680-460H460v-60h219.65q50.14 0 85.25 35 35.1 35 35.1 85v170q0 63-43.5 106.5T650-80H445ZM203-665q-11.07-18.75-17.04-40.49-5.96-21.74-5.96-44.76 0-70.75 49.5-120.25T350-920q71 0 120.5 49.74t49.5 120.3q0 22.96-5.96 44.62Q508.07-683.68 497-665l-52-30q7-12 11-26t4-29.48Q460-796 427.88-828q-32.12-32-78-32T272-827.92q-32 32.09-32 77.92 0 15 4 29t11 26l-52 30Zm285 335Z"
            }
            Sym::PieChart => {
                "M510-510h309q-11-122-98.5-211.5T510-819v309Zm-60 369v-678q-131 11-220.5 109T140-480q0 132 89.5 230T450-141Zm60 0q123-8 210.5-97.5T819-450H510v309Zm-30-339Zm0 400q-83 0-156-31.5T197-197q-54-54-85.5-127T80-480q0-83 31.5-156T197-763q54-54 127-85.5T480-880q83 0 156 31.5T763-763q54 54 85.5 127T880-480q0 83-31.5 156T763-197q-54 54-127 85.5T480-80Z"
            }
            Sym::Lock => {
                "M220-80q-24.75 0-42.37-17.63Q160-115.25 160-140v-434q0-24.75 17.63-42.38Q195.25-634 220-634h70v-96q0-78.85 55.61-134.42Q401.21-920 480.11-920q78.89 0 134.39 55.58Q670-808.85 670-730v96h70q24.75 0 42.38 17.62Q800-598.75 800-574v434q0 24.75-17.62 42.37Q764.75-80 740-80H220Zm0-60h520v-434H220v434Zm314.5-162.03Q557-324.06 557-355q0-30-22.67-54.5t-54.5-24.5q-31.83 0-54.33 24.5t-22.5 55q0 30.5 22.67 52.5t54.5 22q31.83 0 54.33-22.03ZM350-634h260v-96q0-54.17-37.88-92.08-37.88-37.92-92-37.92T388-822.08q-38 37.91-38 92.08v96ZM220-140v-434 434Z"
            }
            Sym::LockClock => {
                "M350-634h260v-93q0-57-37.5-95T480-860q-55 0-92.5 38T350-727v93ZM557-80H220q-23 0-41.5-18.5T160-140v-434q0-23 18.5-41.5T220-634h70v-93q0-81 55-137t135-56q80 0 135 56t55 137v93h70q23 0 41.5 18.5T800-574v87q-13-3-27-4t-33-1v-82H220v434h301q7 17 15 30.5T557-80Zm60-23q-57-57-57-136t57-136q57-57 136-57t136 57q57 57 57 136t-57 136q-57 57-136 57t-136-57Zm210-35 27-27-84-76v-124h-42v135.78L827-138ZM220-574v434-434Z"
            }
            Sym::SwapHoriz => {
                "M273-160 80-353l193-193 42 42-121 121h316v60H194l121 121-42 42Zm414-254-42-42 121-121H450v-60h316L645-758l42-42 193 193-193 193Z"
            }
            Sym::PostAdd => {
                "M180-120q-24 0-42-18t-18-42v-600q0-24 18-42t42-18h394v60H180v600h600v-394h60v394q0 24-18 42t-42 18H180Zm141-157v-60h319v60H321Zm0-127v-60h319v60H321Zm0-127v-60h319v60H321Zm371-73v-88h-88v-60h88v-88h60v88h88v60h-88v88h-60Z"
            }
            Sym::StarShine => {
                "M852-226 746-332l42-42 106 106-42 42ZM708-706l-42-42 106-106 42 42-106 106Zm-456 0L146-812l42-42 106 106-42 42ZM108-226l-42-42 106-106 42 42-106 106Zm215-19 157-94 157 95-42-178 138-120-182-16-71-168-71 167-182 16 138 120-42 178Zm-90 125 65-281L80-590l288-25 112-265 112 265 288 25-218 189 65 281-247-149-247 149Zm247-365Z"
            }
            Sym::LiveTv => {
                "m383-350 267-170-267-170v340Zm-53 230v-80H140q-24 0-42-18t-18-42v-520q0-24 18-42t42-18h680q24 0 42 18t18 42v520q0 24-18 42t-42 18H630v80H330ZM140-260h680v-520H140v520Zm0 0v-520 520Z"
            }
            Sym::Send => {
                "M120-160v-640l760 320-760 320Zm60-93 544-227-544-230v168l242 62-242 60v167Zm0 0v-457 457Z"
            }
            Sym::PhotoCamera => {
                "M479.5-267q72.5 0 121.5-49t49-121.5q0-72.5-49-121T479.5-607q-72.5 0-121 48.5t-48.5 121q0 72.5 48.5 121.5t121 49Zm0-60q-47.5 0-78.5-31.5t-31-79q0-47.5 31-78.5t78.5-31q47.5 0 79 31t31.5 78.5q0 47.5-31.5 79t-79 31.5ZM140-120q-24 0-42-18t-18-42v-513q0-23 18-41.5t42-18.5h147l73-87h240l73 87h147q23 0 41.5 18.5T880-693v513q0 24-18.5 42T820-120H140Zm0-60h680v-513H645l-73-87H388l-73 87H140v513Zm340-257Z"
            }
            Sym::TravelExplore => {
                "M458-81q-79-4-148-37t-120-86.5Q139-258 109.5-329T80-480q0-83 31.5-156T197-763q54-54 127-85.5T480-880q149 0 259 94t135 236h-61q-17-84-71-150t-135-99v18q0 35-24 61t-59 26h-87v87q0 17-13.5 28T393-568h-83v88h110v125h-67L149-559q-5 20-7 39.5t-2 39.5q0 135 91 233t227 106v60Zm392-26L716-241q-21 15-45.5 23t-50.5 8q-71 0-120.5-49.5T450-380q0-71 49.5-120.5T620-550q71 0 120.5 49.5T790-380q0 26-8.5 50.5T759-283l134 133-43 43ZM698-302q32-32 32-78t-32-78q-32-32-78-32t-78 32q-32 32-32 78t32 78q32 32 78 32t78-32Z"
            }
            Sym::Refresh => {
                "M196-331q-20-36-28-72.5t-8-74.5q0-131 94.5-225.5T480-798h43l-80-80 39-39 149 149-149 149-40-40 79-79h-41q-107 0-183.5 76.5T220-478q0 29 5.5 55t13.5 49l-43 43ZM476-40 327-189l149-149 39 39-80 80h45q107 0 183.5-76.5T740-479q0-29-5-55t-15-49l43-43q20 36 28.5 72.5T800-479q0 131-94.5 225.5T480-159h-45l80 80-39 39Z"
            }
            Sym::Home => {
                "M220-180h150v-250h220v250h150v-390L480-765 220-570v390Zm-60 60v-480l320-240 320 240v480H530v-250H430v250H160Zm320-353Z"
            }
        }
    }
}

/// One Material Symbol at `size` CSS px (the mockup's `text-[Npx]`), in `currentColor`.
#[component]
pub fn SymIcon(
    sym: Sym,
    #[props(default = 20)] size: u32,
    #[props(default)] class: String,
) -> Element {
    rsx! {
        svg {
            class: "k-sym {class}",
            "aria-hidden": "true",
            "focusable": "false",
            width: "{size}",
            height: "{size}",
            view_box: "0 -960 960 960",
            fill: "currentColor",
            path { d: sym.path() }
        }
    }
}

/// The L6 icon names, kept for the screens L7-1 has not redesigned yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Home,
    Tipsters,
    Account,
    Back,
    Close,
    Refresh,
}

impl Icon {
    /// The Material Symbol it now draws (the v2 bottom nav's icons for its tabs).
    pub fn sym(self) -> Sym {
        match self {
            Icon::Home => Sym::Soccer,
            Icon::Tipsters => Sym::Group,
            Icon::Account => Sym::Wallet,
            Icon::Back => Sym::ArrowBack,
            Icon::Close => Sym::Close,
            Icon::Refresh => Sym::Refresh,
        }
    }
}

/// One L6 icon at `size` px. `stroke` is ignored (the Material shapes are filled); it stays so
/// the L6 call sites compile unchanged.
#[component]
pub fn Svg(
    icon: Icon,
    #[props(default = 18)] size: u32,
    #[props(default = 2.0)] stroke: f32,
) -> Element {
    let _ = stroke;
    rsx! {
        SymIcon { sym: icon.sym(), size }
    }
}

/// The verified badge (D-304: PRO = verified): the filled Material `verified` rosette, dark on a
/// Solana-green disc with a green glow, as the mockup pins it to avatars. It carries meaning, so
/// it is an image with its own name.
#[component]
pub fn VerifiedBadge(#[props(default = 16)] size: u32) -> Element {
    let glyph = (size * 11 / 16).max(8);
    rsx! {
        span {
            class: "k-verified",
            role: "img",
            aria_label: "Verified tipster",
            title: "Verified tipster",
            style: "width: {size}px; height: {size}px",
            SymIcon { sym: Sym::VerifiedFill, size: glyph }
        }
    }
}

/// The X (Twitter) logo, for "Brag on 𝕏" as the mockup draws it (a brand mark, not a Material
/// Symbol). Decorative.
#[component]
pub fn XLogo(#[props(default = 14)] size: u32) -> Element {
    rsx! {
        svg {
            class: "k-sym",
            "aria-hidden": "true",
            "focusable": "false",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 24 24",
            fill: "currentColor",
            path { d: "M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-5.214-6.817L4.99 21.75H1.68l7.73-8.835L1.254 2.25H8.08l4.713 6.231zm-1.161 17.52h1.833L7.084 4.126H5.117z" }
        }
    }
}
