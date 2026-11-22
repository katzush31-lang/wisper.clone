//! Built-in Hebrew/Yiddish vocabulary in Ashkenazi (Yeshivish) transliteration.
//!
//! Each entry: preferred spelling, alternative spellings that are always safe to replace
//! automatically, sound-alike mis-hearings (only given to the AI as hints, never replaced
//! blindly — "school" or "from" are real English words), and whether to send it to the
//! speech model as a keyword hint.

pub struct Term {
    pub word: &'static str,
    /// Spellings replaced automatically (whole word, any case).
    pub variants: &'static [&'static str],
    /// What a speech model might hear instead; context decides, so these are hints only.
    pub sounds_like: &'static [&'static str],
    /// Send as a keyword hint to speech recognition.
    pub keyword: bool,
}

const fn t(
    word: &'static str,
    variants: &'static [&'static str],
    sounds_like: &'static [&'static str],
    keyword: bool,
) -> Term {
    Term { word, variants, sounds_like, keyword }
}

/// Bump when the list changes so existing installs pick up new entries.
pub const STARTER_VERSION: u32 = 1;

pub const STARTER: &[Term] = &[
    // Phrases with Hashem
    t("Hashem", &["HaShem", "Ha Shem", "Ha-Shem"], &["a shem", "the shem"], true),
    t("b'ezras Hashem", &["beezras Hashem", "b'ezrat Hashem", "beezrat Hashem", "bezras Hashem", "b ezras Hashem", "b'ezras HaShem", "be'ezras Hashem"], &["be as rush a shem", "bez russ Hashem", "the Ezra's Hashem"], true),
    t("be'H", &["b'H", "beH", "B'H"], &["BH", "B.H.", "B H", "bee H"], true),
    t("baruch Hashem", &["boruch Hashem", "baruch HaShem", "boruch HaShem", "baruk Hashem"], &["Baruch a shem", "Brooke Hashem"], true),
    t("B\"H", &[], &[], false),
    t("im yirtzeh Hashem", &["im yirtze Hashem", "im yirtza Hashem", "imyirtzeh Hashem", "im yirtzeh HaShem", "im yirtse Hashem"], &["im year tzeh Hashem", "him here say Hashem"], true),
    t("IY\"H", &["IYH", "I\"YH"], &["I Y H"], false),
    t("bli neder", &["b'li neder", "blee neder", "bli nedder"], &["blini there", "bly neder"], true),
    t("chas v'shalom", &["chas veshalom", "chas v'sholom", "chas vesholom", "chas v'chalila"], &["has veshalom", "has the shalom"], true),
    t("chas v'chalilah", &["chas vechalila", "chas v'chalila"], &[], false),
    t("kiddush Hashem", &["kidush Hashem"], &[], false),
    t("chillul Hashem", &["chilul Hashem"], &["hillel Hashem"], false),
    // Core vocabulary the owner uses
    t("zechus", &["zchus", "zechut", "zchut", "z'chus", "zechuss"], &["Zeus", "the coos", "zeckus", "zachus", "zekus", "Zacchaeus"], true),
    t("zechusim", &["zchusim", "zechuyot", "zchuyos"], &["Zeus seem"], false),
    t("parnasa", &["parnassa", "parnosa", "parnossa", "parnassah", "parnasah", "parnossah", "parnasah"], &["Parnassus", "par nasa", "Parnassa"], true),
    t("nachas", &["naches", "nachat", "nachus", "nakhes"], &["knock us", "knock is", "Nachos"], true),
    t("chessed", &["chesed", "chessid", "hesed"], &["chess said", "chest"], true),
    t("tzedakah", &["tzedaka", "tzedokah", "tsedakah", "tzedoko", "tzedakkah"], &["said aka", "the darker"], true),
    t("mitzvah", &["mitzva", "mitsvah"], &["mits va", "mitts va"], true),
    t("mitzvos", &["mitzvot", "mitzvahs", "mitzvohs"], &[], true),
    t("simcha", &["simchah", "simhah", "simha"], &["sim ha", "simka"], true),
    t("simchas", &["simchat", "simchos", "simchot"], &[], false),
    t("mazel tov", &["mazal tov", "mazeltov", "mazel tof", "mazal tof"], &["Mazel Tov"], true),
    t("hatzlacha", &["hatzlocha", "hatzlachah", "hatzlocho", "hatzlaha"], &["has lacha", "hots locker"], true),
    t("bracha", &["brocha", "beracha", "brachah", "brocho", "berachah"], &["Bracha", "brokha"], true),
    t("brachos", &["brachot", "berachot", "brochos", "berachos"], &[], false),
    t("Shabbos", &["Shabbat", "Shabbas", "Shabbes", "Shabos", "Shobbos"], &["shabby us", "Shabbas"], true),
    t("Shabbos Kodesh", &["Shabbat Kodesh", "Shabbos Koidesh"], &[], false),
    t("Yom Tov", &["yomtov", "yom tof", "yontiff", "yontif"], &["yum tov", "young tov"], true),
    t("Gut Shabbos", &["Good Shabbos", "Gut Shabbes"], &[], false),
    t("gut voch", &["gutte voch", "good voch"], &["good walk"], false),
    t("Chag Sameach", &["chag someach", "chag samayach"], &[], false),
    t("l'chaim", &["lechaim", "l'chayim", "lchaim", "le chaim", "l'chayyim"], &["la hime", "lahayim"], true),
    // Learning and davening
    t("daven", &["doven", "davven"], &["Devon", "Darvin", "dove in"], true),
    t("davening", &["dovening", "davvening"], &["dove ning", "having"], true),
    t("davened", &["dovened"], &[], false),
    t("shul", &["shool"], &["school", "shoal"], true),
    t("yeshiva", &["yeshivah", "yeshivas"], &["the shiva", "yes Shiva"], true),
    t("bochur", &["bachur", "bocher", "bochor"], &["Boker", "bocce"], true),
    t("bochurim", &["bachurim", "bocherim", "bochrim"], &[], false),
    t("kollel", &["kolel", "colel"], &["coal L", "Colel"], true),
    t("shiur", &["shiyur"], &["sure", "shoe are", "she ur"], true),
    t("shiurim", &["shiyurim"], &[], false),
    t("chavrusa", &["chavruta", "chavrusah", "chavrisa", "havrusa", "chavrutah"], &["have Rusa", "half Rusa"], true),
    t("Gemara", &["Gemora", "Gemarah", "Gemorah"], &["the Mara", "gamora"], true),
    t("Mishnah", &["Mishna", "Mishnayos"], &[], false),
    t("Halacha", &["Halachah", "Halakha", "Halakhah"], &["hello ha", "Hala ha"], true),
    t("halachically", &["halakhically"], &[], false),
    t("Chumash", &["Chumosh"], &["who mash"], false),
    t("Tehillim", &["Tehilim", "Tehilem", "Tillim", "Tilim"], &["tea hill eem", "Till em"], true),
    t("tefillah", &["tefila", "tefilla", "tfila", "tfilla", "tefilah"], &["the fila", "tiffiller"], true),
    t("tefillos", &["tefillot", "tefilos", "tfilos"], &[], false),
    t("tefillin", &["tefilin", "tfilin", "tefilen"], &["the Philippine"], false),
    t("Mincha", &["Minchah", "Minha"], &["mink ha"], true),
    t("Maariv", &["Ma'ariv", "Mariv"], &["Ma Reeve"], true),
    t("Shacharis", &["Shacharit", "Shachris", "Shachrit"], &["Sharkey's"], true),
    t("Kiddush", &["kidush", "kiddish"], &["kiddish"], true),
    t("Havdalah", &["Havdala", "Havdoloh"], &[], false),
    t("seudah", &["seuda", "seudoh"], &[], false),
    t("Rebbe", &["rebbeh"], &["Reba", "rabbit"], true),
    t("rebbi", &["rebbie"], &[], false),
    t("Rav", &[], &[], false),
    t("rosh yeshiva", &["rosh yeshivah"], &[], false),
    t("mashgiach", &["mashgiyach"], &["mash geared"], false),
    t("rebbetzin", &["rebbitzen", "rebbetzen", "rebbitzin"], &["rabbits in"], false),
    t("Rabbosai", &["Rabosai"], &[], false),
    t("Torah", &[], &[], false),
    t("Yiddishkeit", &["Yiddishkite", "Yidishkeit"], &["Yiddish kite"], true),
    t("teshuvah", &["teshuva", "tshuva", "tshuvah"], &["the shiva"], true),
    t("kavanah", &["kavana", "kavono", "kavannah"], &["Havana", "cavana"], true),
    t("emunah", &["emuna", "emunoh"], &["a moon a"], true),
    t("bitachon", &["bitochon", "bitahon", "bittachon"], &["bit a hon"], true),
    t("hashgacha", &["hashgachah", "hashgocha"], &[], false),
    t("hashgacha pratis", &["hashgacha pratit", "hashgocha protis"], &[], false),
    t("hishtadlus", &["hishtadlut", "histadlus"], &[], false),
    t("middos", &["midos", "middot", "midot"], &["meadows"], true),
    t("derech eretz", &["derekh eretz"], &[], false),
    t("ahavas Yisroel", &["ahavat Yisrael", "ahavas Yisrael"], &[], false),
    t("lashon hara", &["loshon hora", "lashon hora", "loshon hara", "lashon harah"], &[], false),
    t("yetzer hara", &["yetzer horah", "yetzer hora"], &[], false),
    t("tzaddik", &["tzadik", "tsadik", "tzaddick"], &["sadik"], false),
    t("tzaddekes", &["tzadeikes", "tzadekes"], &[], false),
    t("chizuk", &["chizzuk", "hizuk"], &[], false),
    t("yasher koach", &["yashar koach", "yasher koyach", "yashir koach"], &[], false),
    t("shkoyach", &["shkoach", "sh'koyach"], &[], false),
    t("nisayon", &["nisoyon", "nissayon"], &[], false),
    t("gashmiyus", &["gashmius", "gashmiut"], &[], false),
    t("ruchniyus", &["ruchnius", "ruchniut"], &[], false),
    t("bein hazmanim", &["bein hazmanim'", "bein hazmannim"], &[], false),
    t("zman", &["z'man"], &[], false),
    t("kavod", &["kovod"], &[], false),
    t("chashuv", &["choshuv", "chashuve"], &[], false),
    // Life events
    t("chasunah", &["chasuna", "chasene", "chasana", "chassuna", "chasunoh"], &["has una", "the son of"], true),
    t("kallah", &["kalla", "kaleh"], &["Calla", "Kayla"], true),
    t("chosson", &["chassan", "chatan", "choson"], &["Hassan"], true),
    t("shidduch", &["shiduch", "shiddach"], &["she dooch", "shit dutch"], true),
    t("shidduchim", &["shiduchim"], &[], false),
    t("shadchan", &["shadchen", "shadkhan"], &["shot Kahn"], false),
    t("vort", &[], &["vault", "fort"], false),
    t("aufruf", &["ufruf", "oyfruf", "aufruff"], &[], false),
    t("bar mitzvah", &["bar mitzva"], &[], false),
    t("bas mitzvah", &["bat mitzvah", "bas mitzva"], &[], false),
    t("bris", &["briss"], &["breeze"], false),
    t("upsherin", &["upsherinish", "upshernish"], &[], false),
    t("levaya", &["levayah", "levoya"], &[], false),
    t("shivah", &[], &[], false),
    t("nichum aveilim", &["nichum avelim"], &[], false),
    t("alav hashalom", &["olav hasholom", "alav hasholom"], &[], false),
    t("zichrono livracha", &["zichrono l'vracha", "zichrono livrocho"], &[], false),
    t("shlit\"a", &["shlita", "shlitah"], &[], false),
    // Community and places
    t("kehilla", &["kehila", "kehillah", "k'hilla"], &[], false),
    t("Hatzolah", &["Hatzalah", "Hatzola"], &[], false),
    t("Moshiach", &["Mashiach", "Moshiah"], &["Messiah"], true),
    t("Eretz Yisroel", &["Eretz Yisrael"], &[], true),
    t("Yerushalayim", &["Yerushalaim"], &[], false),
    t("Chazal", &["Chazzal"], &[], false),
    t("frum", &["frim"], &["from"], true),
    t("heimish", &["haimish", "heimishe"], &[], true),
    t("Litvish", &[], &[], false),
    t("Chassidish", &["Chasidish"], &[], false),
    // Festivals
    t("Pesach", &["Pesah"], &["pay sock", "Passover"], true),
    t("Sukkos", &["Sukkot", "Succos", "Sukos"], &["Suckers"], true),
    t("Shavuos", &["Shavuot", "Shavuas"], &[], true),
    t("Chanukah", &["Hanukkah", "Chanuka", "Chanukkah", "Hanukah"], &[], true),
    t("Purim", &[], &[], false),
    t("Rosh Hashanah", &["Rosh Hashana", "Rosh Hashono"], &[], true),
    t("Yom Kippur", &[], &[], false),
    t("Elul", &[], &["a lull"], false),
    // Yiddish expressions
    t("mamesh", &["mamash", "mammash"], &["ma mesh"], true),
    t("tachlis", &["tachles", "tachlit"], &["talk less"], true),
    t("geshmak", &["geshmack", "geshmake"], &["gesh mock"], false),
    t("gevaldig", &["geveldig", "gevaldik"], &[], false),
    t("eppes", &["epes", "eppis"], &[], false),
    t("takeh", &[], &[], false),
    t("stam", &["shtam"], &[], false),
    t("davka", &["dafka"], &["dafka"], false),
    t("nebach", &["nebech", "nebuch"], &[], false),
    t("mensch", &["mentsch", "mentch"], &[], false),
    t("chutzpah", &["chutzpa", "hutzpah"], &[], false),
    t("kvetch", &["kvetsh"], &[], false),
    t("schlep", &["shlep"], &[], false),
    t("oy vey", &["oy vay", "oi vey"], &[], false),
    t("nu", &[], &[], false),
    t("Bubby", &["Bubbie", "Bubbe"], &[], false),
    t("Zeidy", &["Zaidy", "Zaidie", "Zeidie", "Zaidi"], &[], false),
    t("Tatty", &["Tatti"], &[], false),
    t("Abba", &[], &[], false),
    t("Ima", &["Imma"], &[], false),
    t("Yid", &[], &[], false),
    t("Yidden", &["Yiden"], &["yidden"], false),
    t("Kesiva vachasima tova", &["ksiva vachasima tova", "k'siva vachasima tovah"], &[], false),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_is_well_formed() {
        let mut words: Vec<String> = STARTER.iter().map(|t| t.word.to_lowercase()).collect();
        let n = words.len();
        words.sort();
        words.dedup_by(|a, b| a == b);
        assert_eq!(n, words.len(), "duplicate words in starter list");
        for term in STARTER {
            assert!(!term.word.trim().is_empty());
            for v in term.variants {
                assert!(!v.contains('<') && !v.contains('>'));
            }
        }
        assert!(STARTER.iter().filter(|t| t.keyword).count() <= 70);
    }

    #[test]
    fn key_requirements_present() {
        let has = |w: &str| STARTER.iter().any(|t| t.word == w);
        assert!(has("zechus"));
        assert!(has("parnasa"));
        assert!(has("b'ezras Hashem"));
        assert!(has("be'H"));
        let zechus = STARTER.iter().find(|t| t.word == "zechus").unwrap();
        assert!(zechus.sounds_like.contains(&"Zeus"));
        // "Zeus" must never be an automatic replacement (it is only a hint).
        assert!(!zechus.variants.contains(&"Zeus"));
    }
}
