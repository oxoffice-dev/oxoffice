//! Citation run grammars, one per style family (data written as code).

use super::*;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum NativeCitationRun {
    Literal(&'static str, usize),
    Country(usize),
    Number {
        letters: usize,
        digits: usize,
    },
    Year(usize),
    CaseTitle {
        letters: usize,
        digits: usize,
    },
    CreatorLast {
        letters: usize,
        digits: usize,
    },
    CreatorFirst {
        letters: usize,
        digits: usize,
    },
    CreatorMiddle {
        letters: usize,
        digits: usize,
    },
    CreatorSpace {
        middle: bool,
        letters: usize,
        digits: usize,
    },
    SourceTitle {
        letters: usize,
        digits: usize,
        spaces_after_letters: usize,
        spaces_after_digits: usize,
    },
}

// Native citation fragments contain literal language grammar, never cached source values.
pub(super) fn apa_patent_run_grammar(
    locale: &BibliographyFormattingLocale,
) -> Option<(bool, &'static [NativeCitationRun])> {
    use NativeCitationRun::{Country, Literal, Number, Year};
    let numeric = |number| -> Option<(bool, &'static [NativeCitationRun])> {
        match number {
            1025 | 1114 | 2049 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241 | 11265
            | 12289 | 13313 | 14337 | 15361 | 16385 => Some((
                true,
                &[
                    Literal("(", 1),
                    Country(2),
                    Literal(" رقم براءة الاختراع ", 1),
                    Number {
                        letters: 2,
                        digits: 2,
                    },
                    Literal("، ", 1),
                    Year(1),
                    Literal(")", 1),
                ],
            )),
            1026 | 1049 | 1087 | 1088 | 1092 | 2073 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Патент № ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1027 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patent núm. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1028 | 3076 | 5124 => Some((
                true,
                &[
                    Literal("(", 3),
                    Country(3),
                    Literal(" ", 3),
                    Literal("專利號碼", 4),
                    Literal(" ", 3),
                    Number {
                        letters: 3,
                        digits: 3,
                    },
                    Literal(", ", 3),
                    Year(3),
                    Literal(")", 3),
                ],
            )),
            1029 | 1051 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patent č. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1030 | 1031 | 1043 | 1044 | 1070 | 1078 | 1080 | 1083 | 1122 | 1145 | 2055 | 2067
            | 2068 | 3079 | 4103 | 5127 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patentnr. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1032 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Ευρεσιτεχνία Αρ. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1033 | 1055 | 1068 | 1072 | 1073 | 1075 | 1084 | 1117 | 1126 | 1127 | 1129 | 1137
            | 1141 | 1142 | 1143 | 2092 | 2115 | 2143 | 2163 | 3081 | 4105 | 5129 | 6153 | 7177
            | 8201 | 9225 | 10249 | 11273 | 12297 | 13321 | 14345 | 15369 | 16393 | 17417
            | 18441 | 58378 | 58380 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patent No. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1034 | 1140 | 2058 | 3082 | 4106 | 5130 | 6154 | 7178 | 8202 | 9226 | 10250 | 11274
            | 12298 | 13322 | 14346 | 15370 | 16394 | 17418 | 18442 | 19466 | 20490 | 21514 => {
                Some((
                    true,
                    &[
                        Literal("(", 0),
                        Country(0),
                        Literal(" Patente nº ", 0),
                        Number {
                            letters: 0,
                            digits: 0,
                        },
                        Literal(", ", 0),
                        Year(0),
                        Literal(")", 0),
                    ],
                ))
            }
            1035 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patenttinro ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1036 | 2060 | 3084 | 4108 | 5132 | 6156 | 7180 | 8204 | 9228 | 10252 | 11276
            | 12300 | 13324 | 14348 | 15372 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Brevet n° ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1037 => Some((
                true,
                &[
                    Literal("(", 1),
                    Country(2),
                    Literal(" פטנט מס' ", 1),
                    Number {
                        letters: 2,
                        digits: 2,
                    },
                    Literal(", ", 2),
                    Year(2),
                    Literal(")", 1),
                ],
            )),
            1038 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Szabadalom száma: ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1039 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Einkaleyfi nr. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1040 | 2064 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Brevetto n. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1041 => Some((
                true,
                &[
                    Literal("[", 3),
                    Country(3),
                    Literal(" ", 3),
                    Literal("特許番号", 4),
                    Literal(": ", 3),
                    Number {
                        letters: 3,
                        digits: 3,
                    },
                    Literal(", ", 3),
                    Year(3),
                    Literal("]", 3),
                ],
            )),
            1042 => Some((
                true,
                &[
                    Literal("(", 3),
                    Country(3),
                    Literal(" ", 3),
                    Literal("특허권", 5),
                    Literal(" ", 3),
                    Literal("번호", 5),
                    Literal(": ", 3),
                    Number {
                        letters: 3,
                        digits: 3,
                    },
                    Literal(", ", 3),
                    Year(3),
                    Literal(")", 3),
                ],
            )),
            1045 | 1061 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patent nr ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1046 | 2070 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patente Nº ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1047 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patent Nr. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1048 | 2072 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Brevet nr. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1050 | 2074 | 4122 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Br. patenta ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1052 | 1062 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patenta nr. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1053 | 2077 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patentnr ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1054 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("เลขที่สิทธิบัตร", 7),
                    Literal(" ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1056 | 2080 => Some((
                true,
                &[
                    Literal("(", 1),
                    Country(2),
                    Literal(" پیٹینٹ نمبر ", 1),
                    Number {
                        letters: 2,
                        digits: 2,
                    },
                    Literal(", ", 2),
                    Year(2),
                    Literal(")", 1),
                ],
            )),
            1057 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Paten No. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1058 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Патент №", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1059 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Патэнт № ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1060 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Št. patenta ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1063 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patento Nr. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1064 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Патенти рақ. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1065 => Some((
                true,
                &[
                    Literal("(", 1),
                    Country(2),
                    Literal(" ش. حق انحصاري ", 1),
                    Number {
                        letters: 2,
                        digits: 2,
                    },
                    Literal(", ", 2),
                    Year(2),
                    Literal(")", 1),
                ],
            )),
            1066 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Đăng ký Độc quyền Nhãn hiệu Số ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1067 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" ", 0),
                    Literal("Արտոնագիր", 8),
                    Literal(" ", 0),
                    Literal("հմ", 8),
                    Literal(". ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1069 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patent zk. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1071 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Патент бр. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1074 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Nomoro ya Patente. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1076 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Inomb. yepeyitenti ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1077 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Inomb. Yephathenti ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1079 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" ", 0),
                    Literal("პატენტის", 9),
                    Literal(" ", 0),
                    Literal("ნომ", 9),
                    Literal(". ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1081 | 1112 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("पेटेंट", 10),
                    Literal(" ", 6),
                    Literal("क्र", 10),
                    Literal(". ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1082 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Nru tal-Privattiva ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1085 | 1119 | 1120 | 1125 => Some((
                true,
                &[
                    Literal("(", 1),
                    Country(2),
                    Literal(" Patent No. ", 2),
                    Number {
                        letters: 2,
                        digits: 2,
                    },
                    Literal(", ", 2),
                    Year(2),
                    Literal(")", 1),
                ],
            )),
            1086 | 2110 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" No. Paten ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1089 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Hataza Na. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1090 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patent belgisi ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1091 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal("-sonli patent, ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1093 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("পেটেন্ট", 11),
                    Literal(" ", 6),
                    Literal("নম্বর", 11),
                    Literal(" ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1094 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("ਪੇਟੰਟ", 12),
                    Literal(" ", 6),
                    Literal("ਨੰ", 12),
                    Literal(" ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1095 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("પેટન્ટ", 13),
                    Literal(" ", 6),
                    Literal("નં", 13),
                    Literal(". ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1096 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("ପେଟେଣ୍ଟ", 14),
                    Literal(" ", 6),
                    Literal("କ୍ର", 14),
                    Literal(". ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1097 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("காப்புரிமை", 15),
                    Literal(" ", 6),
                    Literal("எண்", 15),
                    Literal(" ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1098 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("పేటెంట్", 16),
                    Literal(" ", 6),
                    Literal("సం", 16),
                    Literal(". ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1099 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("ಪೇಟೆಂಟ್", 17),
                    Literal(" ", 6),
                    Literal("ಸಂಖ್ಯೆ", 17),
                    Literal(". ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1100 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("പേറ്റന്റ്", 18),
                    Literal(" ", 6),
                    Literal("നമ്പര്‍", 18),
                    Literal(" ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1101 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("পেটেন্ট", 11),
                    Literal(" ", 6),
                    Literal("ক্ৰ", 11),
                    Literal(". ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1102 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("पेटंट", 10),
                    Literal(" ", 6),
                    Literal("क्र", 10),
                    Literal(". ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1103 | 1109 | 1113 | 2144 | 2145 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" Patent No. ", 2),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1104 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Патентийн Д.д. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1105 | 2128 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("专", 19),
                    Literal("利号", 4),
                    Literal(" ", 2),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1106 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Rhif Patent ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1107 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("ប៉ាតង់លេខ", 20),
                    Literal(". ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1108 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("ເລກທີສິດ", 21),
                    Literal("\u{200B}", 6),
                    Literal("ທິ", 21),
                    Literal("\u{200B}", 6),
                    Literal("ບັດ", 21),
                    Literal(" ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1110 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patente núm. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1111 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("पेटेंट", 10),
                    Literal(" ", 6),
                    Literal("सं", 10),
                    Literal(". ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1115 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("පේටන්ට්", 22),
                    Literal(". ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1116 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" ", 0),
                    Literal("ᎠᏤᏝᏅ", 23),
                    Literal(" ", 0),
                    Literal("ᏗᏎᏍᏗ", 23),
                    Literal(". ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1118 | 1138 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" ", 0),
                    Literal("የፓ", 24),
                    Literal(". ", 0),
                    Literal("ቁ", 24),
                    Literal(". ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1121 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("विशिष्ट", 10),
                    Literal(" ", 6),
                    Literal("अधिकार", 10),
                    Literal(" ", 6),
                    Literal("पत्र", 10),
                    Literal(" ", 6),
                    Literal("संख्या", 10),
                    Literal(" ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            1123 => Some((
                true,
                &[
                    Literal("(", 1),
                    Country(2),
                    Literal(" ", 2),
                    Number {
                        letters: 2,
                        digits: 2,
                    },
                    Literal("امتیازی حق نمبر, ", 1),
                    Year(1),
                    Literal(")", 1),
                ],
            )),
            1124 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Blg. ng Patent ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1128 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Lambar takaddar izini ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1130 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" NỌ. Aṣẹ ọja tita ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1131 | 2155 | 3179 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Sut'i nº ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1132 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Nomoro ya Phatente No. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1136 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Nọmba Ikike ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1139 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" ", 0),
                    Literal("ቁ", 24),
                    Literal(". ", 0),
                    Literal("ፍቃድ", 24),
                    Literal(" ", 0),
                    Literal("ሓላፍነት", 24),
                    Literal(" ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1144 | 2052 | 4100 => Some((
                true,
                &[
                    Literal("(", 3),
                    Country(3),
                    Literal(" ", 3),
                    Literal("专", 19),
                    Literal("利号", 4),
                    Literal(" ", 3),
                    Number {
                        letters: 3,
                        digits: 3,
                    },
                    Literal(", ", 3),
                    Year(3),
                    Literal(")", 3),
                ],
            )),
            1152 => Some((
                true,
                &[
                    Literal("(", 1),
                    Country(2),
                    Literal(" پاتېنت نومۇرى: ", 1),
                    Number {
                        letters: 2,
                        digits: 2,
                    },
                    Literal(", ", 2),
                    Year(2),
                    Literal(")", 1),
                ],
            )),
            1153 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Tau Tohu tiaki tenenga ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            2057 | 2129 => Some((
                false,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patent No. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            2108 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Uimh. Phaitinne: ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            2117 => Some((
                true,
                &[
                    Literal("(", 6),
                    Country(2),
                    Literal(" ", 2),
                    Literal("পেটেন্ট", 11),
                    Literal("/", 6),
                    Literal("স্বত্ব", 11),
                    Literal(" ", 6),
                    Literal("নং", 11),
                    Literal(". ", 6),
                    Number {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            2118 => Some((
                true,
                &[
                    Literal("(", 1),
                    Country(2),
                    Literal(" پیٹنٹ نمبر ", 1),
                    Number {
                        letters: 2,
                        digits: 2,
                    },
                    Literal(", ", 2),
                    Year(2),
                    Literal(")", 1),
                ],
            )),
            2137 => Some((
                true,
                &[
                    Literal("(", 1),
                    Country(2),
                    Literal(" ظاھري نمبر۔ ", 1),
                    Number {
                        letters: 2,
                        digits: 2,
                    },
                    Literal(", ", 2),
                    Year(2),
                    Literal(")", 1),
                ],
            )),
            3098 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Бр. патента ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            5146 => Some((
                true,
                &[
                    Literal("(", 0),
                    Country(0),
                    Literal(" Patent br. ", 0),
                    Number {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            _ => None,
        }
    };
    match locale {
        BibliographyFormattingLocale::Numeric(number) => numeric(*number),
        BibliographyFormattingLocale::Lexical(name) => {
            let mut choices = SOURCE_LOCALE_ENCODINGS
                .iter()
                .filter(|(_, encoding)| encoding.eq_ignore_ascii_case(name))
                .map(|(number, _)| numeric(*number));
            let first = choices.next()??;
            choices.all(|choice| choice == Some(first)).then_some(first)
        }
    }
}

pub(super) fn apa_case_run_grammar(
    locale: &BibliographyFormattingLocale,
) -> Option<(bool, &'static [NativeCitationRun])> {
    use NativeCitationRun::{CaseTitle, Literal, Year};
    let numeric = |number| -> Option<(bool, &'static [NativeCitationRun])> {
        match number {
            1025 | 1114 | 2049 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241 | 11265
            | 12289 | 13313 | 14337 | 15361 | 16385 => Some((
                true,
                &[
                    Literal("(", 1),
                    CaseTitle {
                        letters: 2,
                        digits: 2,
                    },
                    Literal("، ", 1),
                    Year(1),
                    Literal(")", 1),
                ],
            )),
            1026 | 1027 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1038 | 1039
            | 1040 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050 | 1051 | 1052 | 1053
            | 1055 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062 | 1063 | 1064 | 1066 | 1067 | 1068
            | 1069 | 1070 | 1071 | 1072 | 1073 | 1074 | 1075 | 1076 | 1077 | 1078 | 1079 | 1080
            | 1082 | 1083 | 1084 | 1086 | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1104 | 1106
            | 1110 | 1116 | 1117 | 1118 | 1122 | 1124 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131
            | 1132 | 1136 | 1137 | 1138 | 1139 | 1140 | 1141 | 1142 | 1143 | 1145 | 1153 | 2055
            | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2092 | 2108
            | 2110 | 2115 | 2143 | 2155 | 2163 | 3079 | 3081 | 3082 | 3084 | 3098 | 3179 | 4103
            | 4105 | 4106 | 4108 | 4122 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153 | 6154 | 6156
            | 7177 | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228 | 10249 | 10250
            | 10252 | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321 | 13322 | 13324
            | 14345 | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394 | 17417 | 17418
            | 18441 | 18442 | 19466 | 20490 | 21514 | 58378 | 58380 => Some((
                true,
                &[
                    Literal("(", 0),
                    CaseTitle {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1028 | 1042 | 1144 | 2052 | 3076 | 4100 | 5124 => Some((
                true,
                &[
                    Literal("(", 3),
                    CaseTitle {
                        letters: 3,
                        digits: 3,
                    },
                    Literal(", ", 3),
                    Year(3),
                    Literal(")", 3),
                ],
            )),
            1037 | 1056 | 1065 | 1085 | 1119 | 1120 | 1123 | 1125 | 1152 | 2080 | 2118 | 2137 => {
                Some((
                    true,
                    &[
                        Literal("(", 1),
                        CaseTitle {
                            letters: 2,
                            digits: 2,
                        },
                        Literal(", ", 2),
                        Year(2),
                        Literal(")", 1),
                    ],
                ))
            }
            1041 => Some((
                true,
                &[
                    Literal("[", 3),
                    CaseTitle {
                        letters: 3,
                        digits: 3,
                    },
                    Literal(", ", 3),
                    Year(3),
                    Literal("]", 3),
                ],
            )),
            1054 | 1081 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098 | 1099 | 1100 | 1101 | 1102
            | 1103 | 1105 | 1107 | 1108 | 1109 | 1111 | 1112 | 1113 | 1115 | 1121 | 2117 | 2128
            | 2144 | 2145 => Some((
                true,
                &[
                    Literal("(", 6),
                    CaseTitle {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            2057 | 2129 => Some((
                false,
                &[
                    Literal("(", 0),
                    CaseTitle {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            _ => None,
        }
    };
    match locale {
        BibliographyFormattingLocale::Numeric(number) => numeric(*number),
        BibliographyFormattingLocale::Lexical(name) => {
            let mut choices = SOURCE_LOCALE_ENCODINGS
                .iter()
                .filter(|(_, encoding)| encoding.eq_ignore_ascii_case(name))
                .map(|(number, _)| numeric(*number));
            let first = choices.next()??;
            choices.all(|choice| choice == Some(first)).then_some(first)
        }
    }
}

// Chicago ordinary creator/year grammar from the authenticated full223 native matrix.
// APA ordinary creator/year grammar from unique-creator full223 native controls.
pub(super) fn apa_creator_run_grammar(
    locale: &BibliographyFormattingLocale,
) -> Option<(bool, &'static [NativeCitationRun])> {
    use NativeCitationRun::{CreatorFirst, CreatorLast, Literal, Year};
    let numeric = |number| -> Option<(bool, &'static [NativeCitationRun])> {
        match number {
            1025 | 1114 | 2049 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241 | 11265
            | 12289 | 13313 | 14337 | 15361 | 16385 => Some((
                true,
                &[
                    Literal("(", 1),
                    CreatorLast {
                        letters: 2,
                        digits: 2,
                    },
                    Literal("، ", 1),
                    Year(1),
                    Literal(")", 1),
                ],
            )),
            1026 | 1027 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1038 | 1039
            | 1040 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050 | 1051 | 1052 | 1053
            | 1055 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062 | 1063 | 1064 | 1066 | 1067 | 1068
            | 1069 | 1070 | 1071 | 1072 | 1073 | 1074 | 1075 | 1076 | 1077 | 1078 | 1079 | 1080
            | 1082 | 1083 | 1084 | 1086 | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1104 | 1106
            | 1110 | 1116 | 1117 | 1118 | 1122 | 1124 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131
            | 1132 | 1136 | 1137 | 1138 | 1139 | 1140 | 1141 | 1142 | 1143 | 1145 | 1153 | 2055
            | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2092 | 2108
            | 2110 | 2115 | 2143 | 2155 | 2163 | 3079 | 3081 | 3082 | 3084 | 3098 | 3179 | 4103
            | 4105 | 4106 | 4108 | 4122 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153 | 6154 | 6156
            | 7177 | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228 | 10249 | 10250
            | 10252 | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321 | 13322 | 13324
            | 14345 | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394 | 17417 | 17418
            | 18441 | 18442 | 19466 | 20490 | 21514 | 58378 | 58380 => Some((
                true,
                &[
                    Literal("(", 0),
                    CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1028 | 1144 | 2052 | 3076 | 4100 | 5124 => Some((
                true,
                &[
                    Literal("(", 3),
                    CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    Literal(", ", 3),
                    Year(3),
                    Literal(")", 3),
                ],
            )),
            1037 | 1056 | 1065 | 1085 | 1119 | 1120 | 1123 | 1125 | 1152 | 2080 | 2118 | 2137 => {
                Some((
                    true,
                    &[
                        Literal("(", 1),
                        CreatorLast {
                            letters: 2,
                            digits: 2,
                        },
                        Literal(", ", 2),
                        Year(2),
                        Literal(")", 1),
                    ],
                ))
            }
            1041 => Some((
                true,
                &[
                    Literal("[", 3),
                    CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    Literal(", ", 3),
                    Year(3),
                    Literal("]", 3),
                ],
            )),
            1042 => Some((
                true,
                &[
                    Literal("(", 3),
                    CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    CreatorFirst {
                        letters: 3,
                        digits: 3,
                    },
                    Literal(", ", 3),
                    Year(3),
                    Literal(")", 3),
                ],
            )),
            1054 | 1081 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098 | 1099 | 1100 | 1101 | 1102
            | 1103 | 1105 | 1107 | 1108 | 1109 | 1111 | 1112 | 1113 | 1115 | 1121 | 2117 | 2128
            | 2144 | 2145 => Some((
                true,
                &[
                    Literal("(", 6),
                    CreatorLast {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(", ", 2),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            2057 | 2129 => Some((
                false,
                &[
                    Literal("(", 0),
                    CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(", ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            _ => None,
        }
    };
    match locale {
        BibliographyFormattingLocale::Numeric(number) => numeric(*number),
        BibliographyFormattingLocale::Lexical(name) => {
            let mut choices = SOURCE_LOCALE_ENCODINGS
                .iter()
                .filter(|(_, encoding)| encoding.eq_ignore_ascii_case(name))
                .map(|(number, _)| numeric(*number));
            let first = choices.next()??;
            choices.all(|choice| choice == Some(first)).then_some(first)
        }
    }
}

pub(super) fn chicago_run_grammar(
    locale: &BibliographyFormattingLocale,
) -> Option<(bool, &'static [NativeCitationRun])> {
    use NativeCitationRun::{CreatorFirst, CreatorLast, Literal, Year};
    let numeric = |number| -> Option<(bool, &'static [NativeCitationRun])> {
        match number {
            1025 | 1037 | 1056 | 1065 | 1085 | 1114 | 1119 | 1120 | 1123 | 1125 | 1152 | 2049
            | 2080 | 2118 | 2137 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241
            | 11265 | 12289 | 13313 | 14337 | 15361 | 16385 => Some((
                true,
                &[
                    Literal("(", 1),
                    CreatorLast {
                        letters: 2,
                        digits: 2,
                    },
                    Literal(" ", 2),
                    Year(2),
                    Literal(")", 1),
                ],
            )),
            1026 | 1027 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1038 | 1039
            | 1040 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050 | 1051 | 1052 | 1053
            | 1055 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062 | 1063 | 1064 | 1066 | 1067 | 1068
            | 1069 | 1070 | 1071 | 1072 | 1073 | 1074 | 1075 | 1076 | 1077 | 1078 | 1079 | 1080
            | 1082 | 1083 | 1084 | 1086 | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1104 | 1106
            | 1110 | 1116 | 1117 | 1118 | 1122 | 1124 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131
            | 1132 | 1136 | 1137 | 1138 | 1139 | 1140 | 1141 | 1142 | 1143 | 1145 | 1153 | 2055
            | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2092 | 2108
            | 2110 | 2115 | 2143 | 2155 | 2163 | 3079 | 3081 | 3082 | 3084 | 3098 | 3179 | 4103
            | 4105 | 4106 | 4108 | 4122 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153 | 6154 | 6156
            | 7177 | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228 | 10249 | 10250
            | 10252 | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321 | 13322 | 13324
            | 14345 | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394 | 17417 | 17418
            | 18441 | 18442 | 19466 | 20490 | 21514 | 58378 | 58380 => Some((
                true,
                &[
                    Literal("(", 0),
                    CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(" ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            1028 | 1144 | 2052 | 3076 | 4100 | 5124 => Some((
                true,
                &[
                    Literal("(", 3),
                    CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    Literal(" ", 3),
                    Year(3),
                    Literal(")", 3),
                ],
            )),
            1041 => Some((
                true,
                &[
                    Literal("[", 3),
                    CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    Literal(" ", 3),
                    Year(3),
                    Literal("]", 3),
                ],
            )),
            1042 => Some((
                true,
                &[
                    Literal("(", 3),
                    CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    CreatorFirst {
                        letters: 3,
                        digits: 3,
                    },
                    Literal(" ", 3),
                    Year(3),
                    Literal(")", 3),
                ],
            )),
            1054 | 1081 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098 | 1099 | 1100 | 1101 | 1102
            | 1103 | 1105 | 1107 | 1108 | 1109 | 1111 | 1112 | 1113 | 1115 | 1121 | 2117 | 2128
            | 2144 | 2145 => Some((
                true,
                &[
                    Literal("(", 6),
                    CreatorLast {
                        letters: 2,
                        digits: 6,
                    },
                    Literal(" ", 6),
                    Year(6),
                    Literal(")", 6),
                ],
            )),
            2057 | 2129 => Some((
                false,
                &[
                    Literal("(", 0),
                    CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    Literal(" ", 0),
                    Year(0),
                    Literal(")", 0),
                ],
            )),
            _ => None,
        }
    };
    match locale {
        BibliographyFormattingLocale::Numeric(number) => numeric(*number),
        BibliographyFormattingLocale::Lexical(name) => {
            let mut choices = SOURCE_LOCALE_ENCODINGS
                .iter()
                .filter(|(_, encoding)| encoding.eq_ignore_ascii_case(name))
                .map(|(number, _)| numeric(*number));
            let first = choices.next()??;
            choices.all(|choice| choice == Some(first)).then_some(first)
        }
    }
}

pub(super) fn iso_author_date_run_grammar(
    locale: &BibliographyFormattingLocale,
    title: bool,
) -> Option<(bool, &'static [NativeCitationRun])> {
    let numeric = |number| -> Option<(bool, &'static [NativeCitationRun])> {
        match (title, number) {
            (
                false,
                1025 | 1114 | 2049 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241 | 11265
                | 12289 | 13313 | 14337 | 15361 | 16385,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 1),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 2,
                    },
                    NativeCitationRun::Literal("، ", 1),
                    NativeCitationRun::Year(1),
                    NativeCitationRun::Literal(")", 1),
                ],
            )),
            (
                true,
                1025 | 1114 | 2049 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241 | 11265
                | 12289 | 13313 | 14337 | 15361 | 16385,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 1),
                    NativeCitationRun::SourceTitle {
                        letters: 2,
                        digits: 2,
                        spaces_after_digits: 2,
                        spaces_after_letters: 2,
                    },
                    NativeCitationRun::Literal("، ", 1),
                    NativeCitationRun::Year(1),
                    NativeCitationRun::Literal(")", 1),
                ],
            )),
            (
                false,
                1026 | 1027 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1038 | 1039
                | 1040 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050 | 1051 | 1052 | 1053
                | 1055 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062 | 1063 | 1064 | 1066 | 1067 | 1068
                | 1069 | 1070 | 1071 | 1072 | 1073 | 1074 | 1075 | 1076 | 1077 | 1078 | 1079 | 1080
                | 1082 | 1083 | 1084 | 1086 | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1104 | 1106
                | 1110 | 1116 | 1117 | 1118 | 1122 | 1124 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131
                | 1132 | 1136 | 1137 | 1138 | 1139 | 1140 | 1141 | 1142 | 1143 | 1145 | 1153 | 2055
                | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2092 | 2108
                | 2110 | 2115 | 2143 | 2155 | 2163 | 3079 | 3081 | 3082 | 3084 | 3098 | 3179 | 4103
                | 4105 | 4106 | 4108 | 4122 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153 | 6154 | 6156
                | 7177 | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228 | 10249 | 10250
                | 10252 | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321 | 13322 | 13324
                | 14345 | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394 | 17417 | 17418
                | 18441 | 18442 | 19466 | 20490 | 21514 | 58378 | 58380,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (
                true,
                1026 | 1027 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1038 | 1039
                | 1040 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050 | 1051 | 1052 | 1053
                | 1055 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062 | 1063 | 1064 | 1066 | 1067 | 1068
                | 1069 | 1070 | 1071 | 1072 | 1073 | 1074 | 1075 | 1076 | 1077 | 1078 | 1079 | 1080
                | 1082 | 1083 | 1084 | 1086 | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1104 | 1106
                | 1110 | 1116 | 1117 | 1118 | 1122 | 1124 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131
                | 1132 | 1136 | 1137 | 1138 | 1139 | 1140 | 1141 | 1142 | 1143 | 1145 | 1153 | 2055
                | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2092 | 2108
                | 2110 | 2115 | 2143 | 2155 | 2163 | 3079 | 3081 | 3082 | 3084 | 3098 | 3179 | 4103
                | 4105 | 4106 | 4108 | 4122 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153 | 6154 | 6156
                | 7177 | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228 | 10249 | 10250
                | 10252 | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321 | 13322 | 13324
                | 14345 | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394 | 17417 | 17418
                | 18441 | 18442 | 19466 | 20490 | 21514 | 58378 | 58380,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::SourceTitle {
                        letters: 0,
                        digits: 0,
                        spaces_after_digits: 0,
                        spaces_after_letters: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (false, 1028 | 1144 | 2052 | 3076 | 4100 | 5124) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 3),
                    NativeCitationRun::CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal(", ", 3),
                    NativeCitationRun::Year(3),
                    NativeCitationRun::Literal(")", 3),
                ],
            )),
            (true, 1028 | 1042 | 1144 | 2052 | 3076 | 4100 | 5124) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 3),
                    NativeCitationRun::SourceTitle {
                        letters: 3,
                        digits: 3,
                        spaces_after_digits: 3,
                        spaces_after_letters: 3,
                    },
                    NativeCitationRun::Literal(", ", 3),
                    NativeCitationRun::Year(3),
                    NativeCitationRun::Literal(")", 3),
                ],
            )),
            (
                false,
                1037 | 1056 | 1065 | 1085 | 1119 | 1120 | 1123 | 1125 | 1152 | 2080 | 2118 | 2137,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 1),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 2,
                    },
                    NativeCitationRun::Literal(", ", 2),
                    NativeCitationRun::Year(2),
                    NativeCitationRun::Literal(")", 1),
                ],
            )),
            (
                true,
                1037 | 1056 | 1065 | 1085 | 1119 | 1120 | 1123 | 1125 | 1152 | 2080 | 2118 | 2137,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 1),
                    NativeCitationRun::SourceTitle {
                        letters: 2,
                        digits: 2,
                        spaces_after_digits: 2,
                        spaces_after_letters: 2,
                    },
                    NativeCitationRun::Literal(", ", 2),
                    NativeCitationRun::Year(2),
                    NativeCitationRun::Literal(")", 1),
                ],
            )),
            (false, 1041) => Some((
                true,
                &[
                    NativeCitationRun::Literal("[", 3),
                    NativeCitationRun::CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal(", ", 3),
                    NativeCitationRun::Year(3),
                    NativeCitationRun::Literal("]", 3),
                ],
            )),
            (true, 1041) => Some((
                true,
                &[
                    NativeCitationRun::Literal("[", 3),
                    NativeCitationRun::SourceTitle {
                        letters: 3,
                        digits: 3,
                        spaces_after_digits: 3,
                        spaces_after_letters: 3,
                    },
                    NativeCitationRun::Literal(", ", 3),
                    NativeCitationRun::Year(3),
                    NativeCitationRun::Literal("]", 3),
                ],
            )),
            (false, 1042) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 3),
                    NativeCitationRun::CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::CreatorFirst {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal(", ", 3),
                    NativeCitationRun::Year(3),
                    NativeCitationRun::Literal(")", 3),
                ],
            )),
            (false, 1054) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 6),
                    NativeCitationRun::CreatorFirst {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::CreatorSpace {
                        middle: false,
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::CreatorMiddle {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::CreatorSpace {
                        middle: true,
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::Literal(", ", 2),
                    NativeCitationRun::Year(6),
                    NativeCitationRun::Literal(")", 6),
                ],
            )),
            (
                true,
                1054 | 1081 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098 | 1099 | 1100 | 1101 | 1102
                | 1103 | 1105 | 1107 | 1108 | 1109 | 1111 | 1112 | 1113 | 1115 | 1121 | 2117 | 2128
                | 2144 | 2145,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 6),
                    NativeCitationRun::SourceTitle {
                        letters: 2,
                        digits: 6,
                        spaces_after_digits: 6,
                        spaces_after_letters: 2,
                    },
                    NativeCitationRun::Literal(", ", 2),
                    NativeCitationRun::Year(6),
                    NativeCitationRun::Literal(")", 6),
                ],
            )),
            (
                false,
                1081 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098 | 1099 | 1100 | 1101 | 1102 | 1103
                | 1105 | 1107 | 1108 | 1109 | 1111 | 1112 | 1113 | 1115 | 1121 | 2117 | 2128 | 2144
                | 2145,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 6),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::Literal(", ", 2),
                    NativeCitationRun::Year(6),
                    NativeCitationRun::Literal(")", 6),
                ],
            )),
            (false, 2057 | 2129) => Some((
                false,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (true, 2057 | 2129) => Some((
                false,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::SourceTitle {
                        letters: 0,
                        digits: 0,
                        spaces_after_digits: 0,
                        spaces_after_letters: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            _ => None,
        }
    };
    match locale {
        BibliographyFormattingLocale::Numeric(number) => numeric(*number),
        BibliographyFormattingLocale::Lexical(name) => {
            let mut choices = SOURCE_LOCALE_ENCODINGS
                .iter()
                .filter(|(_, encoding)| encoding.eq_ignore_ascii_case(name))
                .map(|(number, _)| numeric(*number));
            let first = choices.next()??;
            choices.all(|choice| choice == Some(first)).then_some(first)
        }
    }
}

pub(super) fn sist_run_grammar(
    locale: &BibliographyFormattingLocale,
    title: bool,
) -> Option<(bool, &'static [NativeCitationRun])> {
    let numeric = |number| -> Option<(bool, &'static [NativeCitationRun])> {
        match (title, number) {
            (
                false,
                1025 | 1037 | 1056 | 1065 | 1085 | 1114 | 1119 | 1120 | 1123 | 1125 | 1152 | 2049
                | 2080 | 2118 | 2137 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241
                | 11265 | 12289 | 13313 | 14337 | 15361 | 16385,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("（", 25),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 2,
                    },
                    NativeCitationRun::Literal("，", 4),
                    NativeCitationRun::Year(2),
                    NativeCitationRun::Literal("）", 25),
                ],
            )),
            (
                true,
                1025 | 1037 | 1056 | 1065 | 1085 | 1114 | 1119 | 1120 | 1123 | 1125 | 1152 | 2049
                | 2080 | 2118 | 2137 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241
                | 11265 | 12289 | 13313 | 14337 | 15361 | 16385,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("（", 25),
                    NativeCitationRun::SourceTitle {
                        letters: 2,
                        digits: 2,
                        spaces_after_digits: 2,
                        spaces_after_letters: 2,
                    },
                    NativeCitationRun::Literal("，", 4),
                    NativeCitationRun::Year(2),
                    NativeCitationRun::Literal("）", 25),
                ],
            )),
            (
                false,
                1026 | 1027 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1038 | 1039
                | 1040 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050 | 1051 | 1052 | 1053
                | 1055 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062 | 1063 | 1064 | 1066 | 1067 | 1068
                | 1069 | 1070 | 1071 | 1072 | 1073 | 1074 | 1075 | 1076 | 1077 | 1078 | 1079 | 1080
                | 1082 | 1083 | 1084 | 1086 | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1104 | 1106
                | 1110 | 1116 | 1117 | 1118 | 1122 | 1124 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131
                | 1132 | 1136 | 1137 | 1138 | 1139 | 1140 | 1141 | 1142 | 1143 | 1145 | 1153 | 2055
                | 2057 | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2092
                | 2108 | 2110 | 2115 | 2129 | 2143 | 2155 | 2163 | 3079 | 3081 | 3082 | 3084 | 3098
                | 3179 | 4103 | 4105 | 4106 | 4108 | 4122 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153
                | 6154 | 6156 | 7177 | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228
                | 10249 | 10250 | 10252 | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321
                | 13322 | 13324 | 14345 | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394
                | 17417 | 17418 | 18441 | 18442 | 19466 | 20490 | 21514 | 58378 | 58380,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("（", 4),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal("，", 4),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal("）", 4),
                ],
            )),
            (
                true,
                1026 | 1027 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1038 | 1039
                | 1040 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050 | 1051 | 1052 | 1053
                | 1055 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062 | 1063 | 1064 | 1066 | 1067 | 1068
                | 1069 | 1070 | 1071 | 1072 | 1073 | 1074 | 1075 | 1076 | 1077 | 1078 | 1079 | 1080
                | 1082 | 1083 | 1084 | 1086 | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1104 | 1106
                | 1110 | 1116 | 1117 | 1118 | 1122 | 1124 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131
                | 1132 | 1136 | 1137 | 1138 | 1139 | 1140 | 1141 | 1142 | 1143 | 1145 | 1153 | 2055
                | 2057 | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2092
                | 2108 | 2110 | 2115 | 2129 | 2143 | 2155 | 2163 | 3079 | 3081 | 3082 | 3084 | 3098
                | 3179 | 4103 | 4105 | 4106 | 4108 | 4122 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153
                | 6154 | 6156 | 7177 | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228
                | 10249 | 10250 | 10252 | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321
                | 13322 | 13324 | 14345 | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394
                | 17417 | 17418 | 18441 | 18442 | 19466 | 20490 | 21514 | 58378 | 58380,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("（", 4),
                    NativeCitationRun::SourceTitle {
                        letters: 0,
                        digits: 0,
                        spaces_after_digits: 0,
                        spaces_after_letters: 0,
                    },
                    NativeCitationRun::Literal("，", 4),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal("）", 4),
                ],
            )),
            (false, 1028 | 1041 | 1042 | 1144 | 2052 | 3076 | 4100 | 5124) => Some((
                true,
                &[
                    NativeCitationRun::Literal("（", 4),
                    NativeCitationRun::CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal("，", 4),
                    NativeCitationRun::Year(3),
                    NativeCitationRun::Literal("）", 4),
                ],
            )),
            (true, 1028 | 1041 | 1042 | 1144 | 2052 | 3076 | 4100 | 5124) => Some((
                true,
                &[
                    NativeCitationRun::Literal("（", 4),
                    NativeCitationRun::SourceTitle {
                        letters: 3,
                        digits: 3,
                        spaces_after_digits: 3,
                        spaces_after_letters: 3,
                    },
                    NativeCitationRun::Literal("，", 4),
                    NativeCitationRun::Year(3),
                    NativeCitationRun::Literal("）", 4),
                ],
            )),
            (
                false,
                1054 | 1081 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098 | 1099 | 1100 | 1101 | 1102
                | 1103 | 1105 | 1107 | 1108 | 1109 | 1111 | 1112 | 1113 | 1115 | 1121 | 2117 | 2128
                | 2144 | 2145,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("（", 4),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::Literal("，", 4),
                    NativeCitationRun::Year(6),
                    NativeCitationRun::Literal("）", 4),
                ],
            )),
            (
                true,
                1054 | 1081 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098 | 1099 | 1100 | 1101 | 1102
                | 1103 | 1105 | 1107 | 1108 | 1109 | 1111 | 1112 | 1113 | 1115 | 1121 | 2117 | 2128
                | 2144 | 2145,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("（", 4),
                    NativeCitationRun::SourceTitle {
                        letters: 2,
                        digits: 6,
                        spaces_after_digits: 6,
                        spaces_after_letters: 2,
                    },
                    NativeCitationRun::Literal("，", 4),
                    NativeCitationRun::Year(6),
                    NativeCitationRun::Literal("）", 4),
                ],
            )),
            _ => None,
        }
    };
    match locale {
        BibliographyFormattingLocale::Numeric(number) => numeric(*number),
        BibliographyFormattingLocale::Lexical(name) => {
            let mut choices = SOURCE_LOCALE_ENCODINGS
                .iter()
                .filter(|(_, encoding)| encoding.eq_ignore_ascii_case(name))
                .map(|(number, _)| numeric(*number));
            let first = choices.next()??;
            choices.all(|choice| choice == Some(first)).then_some(first)
        }
    }
}

pub(super) fn other_style_citation_run_grammar(
    style: BibliographyStyle,
    locale: &BibliographyFormattingLocale,
    title: bool,
) -> Option<(bool, &'static [NativeCitationRun])> {
    let numeric = |number| -> Option<(bool, &'static [NativeCitationRun])> {
        match (style, title, number) {
            (
                BibliographyStyle::Gb7714,
                false,
                1025 | 1114 | 2049 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241 | 11265
                | 12289 | 13313 | 14337 | 15361 | 16385,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 1),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 2,
                    },
                    NativeCitationRun::Literal("، ", 1),
                    NativeCitationRun::Year(1),
                    NativeCitationRun::Literal(")", 1),
                ],
            )),
            (
                BibliographyStyle::Gb7714,
                true,
                1025 | 1114 | 2049 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241 | 11265
                | 12289 | 13313 | 14337 | 15361 | 16385,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 1),
                    NativeCitationRun::SourceTitle {
                        letters: 2,
                        digits: 2,
                        spaces_after_digits: 2,
                        spaces_after_letters: 2,
                    },
                    NativeCitationRun::Literal("، ", 1),
                    NativeCitationRun::Year(1),
                    NativeCitationRun::Literal(")", 1),
                ],
            )),
            (
                BibliographyStyle::Gb7714,
                false,
                1026 | 1027 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1038 | 1039
                | 1040 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050 | 1051 | 1052 | 1053
                | 1055 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062 | 1063 | 1064 | 1066 | 1067 | 1068
                | 1069 | 1070 | 1071 | 1072 | 1073 | 1074 | 1075 | 1076 | 1077 | 1078 | 1079 | 1080
                | 1082 | 1083 | 1084 | 1086 | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1104 | 1106
                | 1110 | 1116 | 1117 | 1118 | 1122 | 1124 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131
                | 1132 | 1136 | 1137 | 1138 | 1139 | 1140 | 1141 | 1142 | 1143 | 1145 | 1153 | 2055
                | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2092 | 2108
                | 2110 | 2115 | 2143 | 2155 | 2163 | 3079 | 3081 | 3082 | 3084 | 3098 | 3179 | 4103
                | 4105 | 4106 | 4108 | 4122 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153 | 6154 | 6156
                | 7177 | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228 | 10249 | 10250
                | 10252 | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321 | 13322 | 13324
                | 14345 | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394 | 17417 | 17418
                | 18441 | 18442 | 19466 | 20490 | 21514 | 58378 | 58380,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (
                BibliographyStyle::Gb7714,
                true,
                1026 | 1027 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1038 | 1039
                | 1040 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050 | 1051 | 1052 | 1053
                | 1055 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062 | 1063 | 1064 | 1066 | 1067 | 1068
                | 1069 | 1070 | 1071 | 1072 | 1073 | 1074 | 1075 | 1076 | 1077 | 1078 | 1079 | 1080
                | 1082 | 1083 | 1084 | 1086 | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1104 | 1106
                | 1110 | 1116 | 1117 | 1118 | 1122 | 1124 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131
                | 1132 | 1136 | 1137 | 1138 | 1139 | 1140 | 1141 | 1142 | 1143 | 1145 | 1153 | 2055
                | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2092 | 2108
                | 2110 | 2115 | 2143 | 2155 | 2163 | 3079 | 3081 | 3082 | 3084 | 3098 | 3179 | 4103
                | 4105 | 4106 | 4108 | 4122 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153 | 6154 | 6156
                | 7177 | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228 | 10249 | 10250
                | 10252 | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321 | 13322 | 13324
                | 14345 | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394 | 17417 | 17418
                | 18441 | 18442 | 19466 | 20490 | 21514 | 58378 | 58380,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::SourceTitle {
                        letters: 0,
                        digits: 0,
                        spaces_after_digits: 0,
                        spaces_after_letters: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (BibliographyStyle::Gb7714, false, 1028 | 1042 | 1144 | 2052 | 3076 | 4100 | 5124) => {
                Some((
                    true,
                    &[
                        NativeCitationRun::Literal("(", 3),
                        NativeCitationRun::CreatorLast {
                            letters: 3,
                            digits: 3,
                        },
                        NativeCitationRun::Literal(", ", 3),
                        NativeCitationRun::Year(3),
                        NativeCitationRun::Literal(")", 3),
                    ],
                ))
            }
            (BibliographyStyle::Gb7714, true, 1028 | 1042 | 1144 | 2052 | 3076 | 4100 | 5124) => {
                Some((
                    true,
                    &[
                        NativeCitationRun::Literal("(", 3),
                        NativeCitationRun::SourceTitle {
                            letters: 3,
                            digits: 3,
                            spaces_after_digits: 3,
                            spaces_after_letters: 3,
                        },
                        NativeCitationRun::Literal(", ", 3),
                        NativeCitationRun::Year(3),
                        NativeCitationRun::Literal(")", 3),
                    ],
                ))
            }
            (
                BibliographyStyle::Gb7714,
                false,
                1037 | 1056 | 1065 | 1085 | 1119 | 1120 | 1123 | 1125 | 1152 | 2080 | 2118 | 2137,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 1),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 2,
                    },
                    NativeCitationRun::Literal(", ", 2),
                    NativeCitationRun::Year(2),
                    NativeCitationRun::Literal(")", 1),
                ],
            )),
            (
                BibliographyStyle::Gb7714,
                true,
                1037 | 1056 | 1065 | 1085 | 1119 | 1120 | 1123 | 1125 | 1152 | 2080 | 2118 | 2137,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 1),
                    NativeCitationRun::SourceTitle {
                        letters: 2,
                        digits: 2,
                        spaces_after_digits: 2,
                        spaces_after_letters: 2,
                    },
                    NativeCitationRun::Literal(", ", 2),
                    NativeCitationRun::Year(2),
                    NativeCitationRun::Literal(")", 1),
                ],
            )),
            (BibliographyStyle::Gb7714, false, 1041) => Some((
                true,
                &[
                    NativeCitationRun::Literal("[", 3),
                    NativeCitationRun::CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal(", ", 3),
                    NativeCitationRun::Year(3),
                    NativeCitationRun::Literal("]", 3),
                ],
            )),
            (BibliographyStyle::Gb7714, true, 1041) => Some((
                true,
                &[
                    NativeCitationRun::Literal("[", 3),
                    NativeCitationRun::SourceTitle {
                        letters: 3,
                        digits: 3,
                        spaces_after_digits: 3,
                        spaces_after_letters: 3,
                    },
                    NativeCitationRun::Literal(", ", 3),
                    NativeCitationRun::Year(3),
                    NativeCitationRun::Literal("]", 3),
                ],
            )),
            (
                BibliographyStyle::Gb7714,
                false,
                1054 | 1081 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098 | 1099 | 1100 | 1101 | 1102
                | 1103 | 1105 | 1107 | 1108 | 1109 | 1111 | 1112 | 1113 | 1115 | 1121 | 2117 | 2128
                | 2144 | 2145,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 6),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::Literal(", ", 2),
                    NativeCitationRun::Year(6),
                    NativeCitationRun::Literal(")", 6),
                ],
            )),
            (
                BibliographyStyle::Gb7714,
                true,
                1054 | 1081 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098 | 1099 | 1100 | 1101 | 1102
                | 1103 | 1105 | 1107 | 1108 | 1109 | 1111 | 1112 | 1113 | 1115 | 1121 | 2117 | 2128
                | 2144 | 2145,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 6),
                    NativeCitationRun::SourceTitle {
                        letters: 2,
                        digits: 6,
                        spaces_after_digits: 6,
                        spaces_after_letters: 2,
                    },
                    NativeCitationRun::Literal(", ", 2),
                    NativeCitationRun::Year(6),
                    NativeCitationRun::Literal(")", 6),
                ],
            )),
            (BibliographyStyle::Gb7714, false, 2057 | 2129) => Some((
                false,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (BibliographyStyle::Gb7714, true, 2057 | 2129) => Some((
                false,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::SourceTitle {
                        letters: 0,
                        digits: 0,
                        spaces_after_digits: 0,
                        spaces_after_letters: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (
                BibliographyStyle::GostName,
                false,
                1025 | 1114 | 2049 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241 | 11265
                | 12289 | 13313 | 14337 | 15361 | 16385,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 1),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 2,
                    },
                    NativeCitationRun::Literal("، ", 1),
                    NativeCitationRun::Year(1),
                    NativeCitationRun::Literal(")", 1),
                ],
            )),
            (
                BibliographyStyle::GostName,
                false,
                1026 | 1027 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1038 | 1039
                | 1040 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050 | 1051 | 1052 | 1053
                | 1055 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062 | 1063 | 1064 | 1066 | 1067 | 1068
                | 1069 | 1070 | 1071 | 1072 | 1073 | 1074 | 1075 | 1076 | 1077 | 1078 | 1079 | 1080
                | 1082 | 1083 | 1084 | 1086 | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1104 | 1106
                | 1110 | 1116 | 1117 | 1118 | 1122 | 1124 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131
                | 1132 | 1136 | 1137 | 1138 | 1139 | 1140 | 1141 | 1142 | 1143 | 1145 | 1153 | 2055
                | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2092 | 2108
                | 2110 | 2115 | 2143 | 2155 | 2163 | 3079 | 3081 | 3082 | 3084 | 3098 | 3179 | 4103
                | 4105 | 4106 | 4108 | 4122 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153 | 6154 | 6156
                | 7177 | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228 | 10249 | 10250
                | 10252 | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321 | 13322 | 13324
                | 14345 | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394 | 17417 | 17418
                | 18441 | 18442 | 19466 | 20490 | 21514 | 58378 | 58380,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (BibliographyStyle::GostName, false, 1028 | 1144 | 2052 | 3076 | 4100 | 5124) => {
                Some((
                    true,
                    &[
                        NativeCitationRun::Literal("(", 3),
                        NativeCitationRun::CreatorLast {
                            letters: 3,
                            digits: 3,
                        },
                        NativeCitationRun::Literal(", ", 3),
                        NativeCitationRun::Year(3),
                        NativeCitationRun::Literal(")", 3),
                    ],
                ))
            }
            (
                BibliographyStyle::GostName,
                false,
                1037 | 1056 | 1065 | 1085 | 1119 | 1120 | 1123 | 1125 | 1152 | 2080 | 2118 | 2137,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 1),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 2,
                    },
                    NativeCitationRun::Literal(", ", 2),
                    NativeCitationRun::Year(2),
                    NativeCitationRun::Literal(")", 1),
                ],
            )),
            (BibliographyStyle::GostName, false, 1041) => Some((
                true,
                &[
                    NativeCitationRun::Literal("[", 3),
                    NativeCitationRun::CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal(", ", 3),
                    NativeCitationRun::Year(3),
                    NativeCitationRun::Literal("]", 3),
                ],
            )),
            (BibliographyStyle::GostName, false, 1042) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 3),
                    NativeCitationRun::CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::CreatorFirst {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal(", ", 3),
                    NativeCitationRun::Year(3),
                    NativeCitationRun::Literal(")", 3),
                ],
            )),
            (BibliographyStyle::GostName, false, 1054) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 6),
                    NativeCitationRun::CreatorFirst {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::CreatorSpace {
                        middle: false,
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::CreatorMiddle {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::CreatorSpace {
                        middle: true,
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::Literal(", ", 2),
                    NativeCitationRun::Year(6),
                    NativeCitationRun::Literal(")", 6),
                ],
            )),
            (
                BibliographyStyle::GostName,
                false,
                1081 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098 | 1099 | 1100 | 1101 | 1102 | 1103
                | 1105 | 1107 | 1108 | 1109 | 1111 | 1112 | 1113 | 1115 | 1121 | 2117 | 2128 | 2144
                | 2145,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 6),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::Literal(", ", 2),
                    NativeCitationRun::Year(6),
                    NativeCitationRun::Literal(")", 6),
                ],
            )),
            (BibliographyStyle::GostName, false, 2057 | 2129) => Some((
                false,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (
                BibliographyStyle::GostTitle,
                false,
                1025 | 1114 | 2049 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241 | 11265
                | 12289 | 13313 | 14337 | 15361 | 16385,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 1),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 2,
                    },
                    NativeCitationRun::Literal("، ", 1),
                    NativeCitationRun::Year(1),
                    NativeCitationRun::Literal(")", 1),
                ],
            )),
            (
                BibliographyStyle::GostTitle,
                false,
                1026 | 1027 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1038 | 1039
                | 1040 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050 | 1051 | 1052 | 1053
                | 1055 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062 | 1063 | 1064 | 1066 | 1067 | 1068
                | 1069 | 1070 | 1071 | 1072 | 1073 | 1074 | 1075 | 1076 | 1077 | 1078 | 1079 | 1080
                | 1082 | 1083 | 1084 | 1086 | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1104 | 1106
                | 1110 | 1116 | 1117 | 1118 | 1122 | 1124 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131
                | 1132 | 1136 | 1137 | 1138 | 1139 | 1140 | 1141 | 1142 | 1143 | 1145 | 1153 | 2055
                | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2092 | 2108
                | 2110 | 2115 | 2143 | 2155 | 2163 | 3079 | 3081 | 3082 | 3084 | 3098 | 3179 | 4103
                | 4105 | 4106 | 4108 | 4122 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153 | 6154 | 6156
                | 7177 | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228 | 10249 | 10250
                | 10252 | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321 | 13322 | 13324
                | 14345 | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394 | 17417 | 17418
                | 18441 | 18442 | 19466 | 20490 | 21514 | 58378 | 58380,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (BibliographyStyle::GostTitle, false, 1028 | 1144 | 2052 | 3076 | 4100 | 5124) => {
                Some((
                    true,
                    &[
                        NativeCitationRun::Literal("(", 3),
                        NativeCitationRun::CreatorLast {
                            letters: 3,
                            digits: 3,
                        },
                        NativeCitationRun::Literal(", ", 3),
                        NativeCitationRun::Year(3),
                        NativeCitationRun::Literal(")", 3),
                    ],
                ))
            }
            (
                BibliographyStyle::GostTitle,
                false,
                1037 | 1056 | 1065 | 1085 | 1119 | 1120 | 1123 | 1125 | 1152 | 2080 | 2118 | 2137,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 1),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 2,
                    },
                    NativeCitationRun::Literal(", ", 2),
                    NativeCitationRun::Year(2),
                    NativeCitationRun::Literal(")", 1),
                ],
            )),
            (BibliographyStyle::GostTitle, false, 1041) => Some((
                true,
                &[
                    NativeCitationRun::Literal("[", 3),
                    NativeCitationRun::CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal(", ", 3),
                    NativeCitationRun::Year(3),
                    NativeCitationRun::Literal("]", 3),
                ],
            )),
            (BibliographyStyle::GostTitle, false, 1042) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 3),
                    NativeCitationRun::CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::CreatorFirst {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal(", ", 3),
                    NativeCitationRun::Year(3),
                    NativeCitationRun::Literal(")", 3),
                ],
            )),
            (BibliographyStyle::GostTitle, false, 1054) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 6),
                    NativeCitationRun::CreatorFirst {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::CreatorSpace {
                        middle: false,
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::CreatorMiddle {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::CreatorSpace {
                        middle: true,
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::Literal(", ", 2),
                    NativeCitationRun::Year(6),
                    NativeCitationRun::Literal(")", 6),
                ],
            )),
            (
                BibliographyStyle::GostTitle,
                false,
                1081 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098 | 1099 | 1100 | 1101 | 1102 | 1103
                | 1105 | 1107 | 1108 | 1109 | 1111 | 1112 | 1113 | 1115 | 1121 | 2117 | 2128 | 2144
                | 2145,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 6),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::Literal(", ", 2),
                    NativeCitationRun::Year(6),
                    NativeCitationRun::Literal(")", 6),
                ],
            )),
            (BibliographyStyle::GostTitle, false, 2057 | 2129) => Some((
                false,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (
                BibliographyStyle::HarvardAnglia,
                false,
                1025 | 1114 | 2049 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241 | 11265
                | 12289 | 13313 | 14337 | 15361 | 16385,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal("، ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (
                BibliographyStyle::HarvardAnglia,
                true,
                1025 | 1114 | 2049 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241 | 11265
                | 12289 | 13313 | 14337 | 15361 | 16385,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::SourceTitle {
                        letters: 0,
                        digits: 0,
                        spaces_after_digits: 0,
                        spaces_after_letters: 0,
                    },
                    NativeCitationRun::Literal("، ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (
                BibliographyStyle::HarvardAnglia,
                false,
                1026 | 1027 | 1028 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1037
                | 1038 | 1039 | 1040 | 1042 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050
                | 1051 | 1052 | 1053 | 1054 | 1055 | 1056 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062
                | 1063 | 1064 | 1065 | 1066 | 1067 | 1068 | 1069 | 1070 | 1071 | 1072 | 1073 | 1074
                | 1075 | 1076 | 1077 | 1078 | 1079 | 1080 | 1081 | 1082 | 1083 | 1084 | 1085 | 1086
                | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098
                | 1099 | 1100 | 1101 | 1102 | 1103 | 1104 | 1105 | 1106 | 1107 | 1108 | 1109 | 1110
                | 1111 | 1112 | 1113 | 1115 | 1116 | 1117 | 1118 | 1119 | 1120 | 1121 | 1122 | 1123
                | 1124 | 1125 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131 | 1132 | 1136 | 1137 | 1138
                | 1139 | 1140 | 1141 | 1142 | 1143 | 1144 | 1145 | 1152 | 1153 | 2052 | 2055 | 2057
                | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2080 | 2092
                | 2108 | 2110 | 2115 | 2117 | 2118 | 2128 | 2129 | 2137 | 2143 | 2144 | 2145 | 2155
                | 2163 | 3076 | 3079 | 3081 | 3082 | 3084 | 3098 | 3179 | 4100 | 4103 | 4105 | 4106
                | 4108 | 4122 | 5124 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153 | 6154 | 6156 | 7177
                | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228 | 10249 | 10250 | 10252
                | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321 | 13322 | 13324 | 14345
                | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394 | 17417 | 17418 | 18441
                | 18442 | 19466 | 20490 | 21514 | 58378 | 58380,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (
                BibliographyStyle::HarvardAnglia,
                true,
                1026 | 1027 | 1028 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1037
                | 1038 | 1039 | 1040 | 1042 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050
                | 1051 | 1052 | 1053 | 1054 | 1055 | 1056 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062
                | 1063 | 1064 | 1065 | 1066 | 1067 | 1068 | 1069 | 1070 | 1071 | 1072 | 1073 | 1074
                | 1075 | 1076 | 1077 | 1078 | 1079 | 1080 | 1081 | 1082 | 1083 | 1084 | 1085 | 1086
                | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098
                | 1099 | 1100 | 1101 | 1102 | 1103 | 1104 | 1105 | 1106 | 1107 | 1108 | 1109 | 1110
                | 1111 | 1112 | 1113 | 1115 | 1116 | 1117 | 1118 | 1119 | 1120 | 1121 | 1122 | 1123
                | 1124 | 1125 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131 | 1132 | 1136 | 1137 | 1138
                | 1139 | 1140 | 1141 | 1142 | 1143 | 1144 | 1145 | 1152 | 1153 | 2052 | 2055 | 2057
                | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2080 | 2092
                | 2108 | 2110 | 2115 | 2117 | 2118 | 2128 | 2129 | 2137 | 2143 | 2144 | 2145 | 2155
                | 2163 | 3076 | 3079 | 3081 | 3082 | 3084 | 3098 | 3179 | 4100 | 4103 | 4105 | 4106
                | 4108 | 4122 | 5124 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153 | 6154 | 6156 | 7177
                | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228 | 10249 | 10250 | 10252
                | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321 | 13322 | 13324 | 14345
                | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394 | 17417 | 17418 | 18441
                | 18442 | 19466 | 20490 | 21514 | 58378 | 58380,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::SourceTitle {
                        letters: 0,
                        digits: 0,
                        spaces_after_digits: 0,
                        spaces_after_letters: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (BibliographyStyle::HarvardAnglia, false, 1041) => Some((
                true,
                &[
                    NativeCitationRun::Literal("[", 0),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal("]", 0),
                ],
            )),
            (BibliographyStyle::HarvardAnglia, true, 1041) => Some((
                true,
                &[
                    NativeCitationRun::Literal("[", 0),
                    NativeCitationRun::SourceTitle {
                        letters: 0,
                        digits: 0,
                        spaces_after_digits: 0,
                        spaces_after_letters: 0,
                    },
                    NativeCitationRun::Literal(", ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal("]", 0),
                ],
            )),
            (
                BibliographyStyle::MlaSeventhEdition,
                false,
                1025 | 1037 | 1056 | 1065 | 1085 | 1114 | 1119 | 1120 | 1123 | 1125 | 1152 | 2049
                | 2080 | 2118 | 2137 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241
                | 11265 | 12289 | 13313 | 14337 | 15361 | 16385,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 1),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 2,
                    },
                    NativeCitationRun::Literal(")", 1),
                ],
            )),
            (
                BibliographyStyle::MlaSeventhEdition,
                true,
                1025 | 1037 | 1056 | 1065 | 1085 | 1114 | 1119 | 1120 | 1123 | 1125 | 1152 | 2049
                | 2080 | 2118 | 2137 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241
                | 11265 | 12289 | 13313 | 14337 | 15361 | 16385,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 1),
                    NativeCitationRun::CaseTitle {
                        letters: 2,
                        digits: 2,
                    },
                    NativeCitationRun::Literal(")", 1),
                ],
            )),
            (
                BibliographyStyle::MlaSeventhEdition,
                false,
                1026 | 1027 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1038 | 1039
                | 1040 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050 | 1051 | 1052 | 1053
                | 1055 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062 | 1063 | 1064 | 1066 | 1067 | 1068
                | 1069 | 1070 | 1071 | 1072 | 1073 | 1074 | 1075 | 1076 | 1077 | 1078 | 1079 | 1080
                | 1082 | 1083 | 1084 | 1086 | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1104 | 1106
                | 1110 | 1116 | 1117 | 1118 | 1122 | 1124 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131
                | 1132 | 1136 | 1137 | 1138 | 1139 | 1140 | 1141 | 1142 | 1143 | 1145 | 1153 | 2055
                | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2092 | 2108
                | 2110 | 2115 | 2143 | 2155 | 2163 | 3079 | 3081 | 3082 | 3084 | 3098 | 3179 | 4103
                | 4105 | 4106 | 4108 | 4122 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153 | 6154 | 6156
                | 7177 | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228 | 10249 | 10250
                | 10252 | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321 | 13322 | 13324
                | 14345 | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394 | 17417 | 17418
                | 18441 | 18442 | 19466 | 20490 | 21514 | 58378 | 58380,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (
                BibliographyStyle::MlaSeventhEdition,
                true,
                1026 | 1027 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1038 | 1039
                | 1040 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050 | 1051 | 1052 | 1053
                | 1055 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062 | 1063 | 1064 | 1066 | 1067 | 1068
                | 1069 | 1070 | 1071 | 1072 | 1073 | 1074 | 1075 | 1076 | 1077 | 1078 | 1079 | 1080
                | 1082 | 1083 | 1084 | 1086 | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1104 | 1106
                | 1110 | 1116 | 1117 | 1118 | 1122 | 1124 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131
                | 1132 | 1136 | 1137 | 1138 | 1139 | 1140 | 1141 | 1142 | 1143 | 1145 | 1153 | 2055
                | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2092 | 2108
                | 2110 | 2115 | 2143 | 2155 | 2163 | 3079 | 3081 | 3082 | 3084 | 3098 | 3179 | 4103
                | 4105 | 4106 | 4108 | 4122 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153 | 6154 | 6156
                | 7177 | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228 | 10249 | 10250
                | 10252 | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321 | 13322 | 13324
                | 14345 | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394 | 17417 | 17418
                | 18441 | 18442 | 19466 | 20490 | 21514 | 58378 | 58380,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CaseTitle {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (
                BibliographyStyle::MlaSeventhEdition,
                false,
                1028 | 1144 | 2052 | 3076 | 4100 | 5124,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 3),
                    NativeCitationRun::CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal(")", 3),
                ],
            )),
            (
                BibliographyStyle::MlaSeventhEdition,
                true,
                1028 | 1042 | 1144 | 2052 | 3076 | 4100 | 5124,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 3),
                    NativeCitationRun::CaseTitle {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal(")", 3),
                ],
            )),
            (BibliographyStyle::MlaSeventhEdition, false, 1041) => Some((
                true,
                &[
                    NativeCitationRun::Literal("[", 3),
                    NativeCitationRun::CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal("]", 3),
                ],
            )),
            (BibliographyStyle::MlaSeventhEdition, true, 1041) => Some((
                true,
                &[
                    NativeCitationRun::Literal("[", 3),
                    NativeCitationRun::CaseTitle {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal("]", 3),
                ],
            )),
            (BibliographyStyle::MlaSeventhEdition, false, 1042) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 3),
                    NativeCitationRun::CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::CreatorFirst {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal(")", 3),
                ],
            )),
            (
                BibliographyStyle::MlaSeventhEdition,
                false,
                1054 | 1081 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098 | 1099 | 1100 | 1101 | 1102
                | 1103 | 1105 | 1107 | 1108 | 1109 | 1111 | 1112 | 1113 | 1115 | 1121 | 2117 | 2128
                | 2144 | 2145,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 6),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::Literal(")", 6),
                ],
            )),
            (
                BibliographyStyle::MlaSeventhEdition,
                true,
                1054 | 1081 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098 | 1099 | 1100 | 1101 | 1102
                | 1103 | 1105 | 1107 | 1108 | 1109 | 1111 | 1112 | 1113 | 1115 | 1121 | 2117 | 2128
                | 2144 | 2145,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 6),
                    NativeCitationRun::CaseTitle {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::Literal(")", 6),
                ],
            )),
            (BibliographyStyle::MlaSeventhEdition, false, 2057 | 2129) => Some((
                false,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (BibliographyStyle::MlaSeventhEdition, true, 2057 | 2129) => Some((
                false,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CaseTitle {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (
                BibliographyStyle::Turabian,
                false,
                1025 | 1037 | 1056 | 1065 | 1085 | 1114 | 1119 | 1120 | 1123 | 1125 | 1152 | 2049
                | 2080 | 2118 | 2137 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241
                | 11265 | 12289 | 13313 | 14337 | 15361 | 16385,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 1),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 2,
                    },
                    NativeCitationRun::Literal(" ", 2),
                    NativeCitationRun::Year(2),
                    NativeCitationRun::Literal(")", 1),
                ],
            )),
            (
                BibliographyStyle::Turabian,
                false,
                1026 | 1027 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1038 | 1039
                | 1040 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050 | 1051 | 1052 | 1053
                | 1055 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062 | 1063 | 1064 | 1066 | 1067 | 1068
                | 1069 | 1070 | 1071 | 1072 | 1073 | 1074 | 1075 | 1076 | 1077 | 1078 | 1079 | 1080
                | 1082 | 1083 | 1084 | 1086 | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1104 | 1106
                | 1110 | 1116 | 1117 | 1118 | 1122 | 1124 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131
                | 1132 | 1136 | 1137 | 1138 | 1139 | 1140 | 1141 | 1142 | 1143 | 1145 | 1153 | 2055
                | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2092 | 2108
                | 2110 | 2115 | 2143 | 2155 | 2163 | 3079 | 3081 | 3082 | 3084 | 3098 | 3179 | 4103
                | 4105 | 4106 | 4108 | 4122 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153 | 6154 | 6156
                | 7177 | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228 | 10249 | 10250
                | 10252 | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321 | 13322 | 13324
                | 14345 | 14346 | 14348 | 15369 | 15370 | 15372 | 16393 | 16394 | 17417 | 17418
                | 18441 | 18442 | 19466 | 20490 | 21514 | 58378 | 58380,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(" ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            (BibliographyStyle::Turabian, false, 1028 | 1144 | 2052 | 3076 | 4100 | 5124) => {
                Some((
                    true,
                    &[
                        NativeCitationRun::Literal("(", 3),
                        NativeCitationRun::CreatorLast {
                            letters: 3,
                            digits: 3,
                        },
                        NativeCitationRun::Literal(" ", 3),
                        NativeCitationRun::Year(3),
                        NativeCitationRun::Literal(")", 3),
                    ],
                ))
            }
            (BibliographyStyle::Turabian, false, 1041) => Some((
                true,
                &[
                    NativeCitationRun::Literal("[", 3),
                    NativeCitationRun::CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal(" ", 3),
                    NativeCitationRun::Year(3),
                    NativeCitationRun::Literal("]", 3),
                ],
            )),
            (BibliographyStyle::Turabian, false, 1042) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 3),
                    NativeCitationRun::CreatorLast {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::CreatorFirst {
                        letters: 3,
                        digits: 3,
                    },
                    NativeCitationRun::Literal(" ", 3),
                    NativeCitationRun::Year(3),
                    NativeCitationRun::Literal(")", 3),
                ],
            )),
            (
                BibliographyStyle::Turabian,
                false,
                1054 | 1081 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098 | 1099 | 1100 | 1101 | 1102
                | 1103 | 1105 | 1107 | 1108 | 1109 | 1111 | 1112 | 1113 | 1115 | 1121 | 2117 | 2128
                | 2144 | 2145,
            ) => Some((
                true,
                &[
                    NativeCitationRun::Literal("(", 6),
                    NativeCitationRun::CreatorLast {
                        letters: 2,
                        digits: 6,
                    },
                    NativeCitationRun::Literal(" ", 6),
                    NativeCitationRun::Year(6),
                    NativeCitationRun::Literal(")", 6),
                ],
            )),
            (BibliographyStyle::Turabian, false, 2057 | 2129) => Some((
                false,
                &[
                    NativeCitationRun::Literal("(", 0),
                    NativeCitationRun::CreatorLast {
                        letters: 0,
                        digits: 0,
                    },
                    NativeCitationRun::Literal(" ", 0),
                    NativeCitationRun::Year(0),
                    NativeCitationRun::Literal(")", 0),
                ],
            )),
            _ => None,
        }
    };
    match locale {
        BibliographyFormattingLocale::Numeric(number) => numeric(*number),
        BibliographyFormattingLocale::Lexical(name) => {
            let mut choices = SOURCE_LOCALE_ENCODINGS
                .iter()
                .filter(|(_, encoding)| encoding.eq_ignore_ascii_case(name))
                .map(|(number, _)| numeric(*number));
            let first = choices.next()??;
            choices.all(|choice| choice == Some(first)).then_some(first)
        }
    }
}
