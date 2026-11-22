// SPDX-License-Identifier: Apache-2.0
//! Raw, case-sensitive wildcard workloads. No equality/CIDR substitutions.

#[derive(Clone, Copy)]
pub enum Family {
    Domains,
    Urls,
    Paths,
    Commands,
}

pub struct Case {
    pub wildcard: String,
    pub positive: String,
    pub negative: String,
    pub shape: &'static str,
}

impl Family {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "domains" => Self::Domains,
            "urls" => Self::Urls,
            "paths" => Self::Paths,
            "commands" => Self::Commands,
            _ => return None,
        })
    }

    pub fn background(self) -> (String, &'static str) {
        match self {
            Self::Domains => ("benign-".into(), ".example.net"),
            Self::Urls => ("https://benign.example/path/".into(), ""),
            Self::Paths => (r"C:\Windows\System32\tool".into(), ".dll"),
            Self::Commands => ("/usr/bin/backup --job ".into(), ""),
        }
    }

    pub fn case(self, id: usize, marker: &str) -> Case {
        let (wildcard, positive, negative, shape) = match (self, id % 3) {
            (Self::Domains, 0) => (
                format!("*.ioc{id:04}.example"),
                format!("login.ioc{id:04}.example"),
                if id.is_multiple_of(6) {
                    format!("ioc{id:04}.example")
                } else {
                    format!("login.ioc{id:04}.example.evil")
                },
                "suffix",
            ),
            (Self::Domains, 1) => (
                format!("login-*.ioc{id:04}.example"),
                format!("login-prod.ioc{id:04}.example"),
                format!("notlogin-prod.ioc{id:04}.example"),
                "interior",
            ),
            (Self::Domains, _) => (
                format!("node-??.ioc{id:04}.example"),
                format!("node-ab.ioc{id:04}.example"),
                format!("node-abc.ioc{id:04}.example"),
                "fixed_width",
            ),
            (Self::Urls, 0) => (
                format!("*/wp-admin/{marker}/*"),
                format!("https://host.example/wp-admin/{marker}/login.php"),
                format!("https://host.example/wp-adminx/{marker}/login.php"),
                "infix",
            ),
            (Self::Urls, 1) => (
                format!("*token={marker}????*"),
                format!("https://host.example/?token={marker}AB12&x=1"),
                format!("https://host.example/?token={marker}ABC"),
                "fixed_width",
            ),
            (Self::Urls, _) => (
                format!("https://*.example/download/{marker}.*"),
                format!("https://cdn.example/download/{marker}.exe"),
                format!("http://cdn.example/download/{marker}.exe"),
                "interior",
            ),
            (Self::Paths, 0) => (
                format!(r"C:\\Users\\*\\Temp\\{marker}*.exe"),
                format!(r"C:\Users\alice\Temp\{marker}-setup.exe"),
                format!(r"C:\Users\alice\Temp\{marker}-setup.exe.bak"),
                "interior",
            ),
            (Self::Paths, 1) => (
                format!(r"C:\\ProgramData\\{marker}\\??.dll"),
                format!(r"C:\ProgramData\{marker}\ab.dll"),
                format!(r"C:\ProgramData\{marker}\abc.dll"),
                "fixed_width",
            ),
            (Self::Paths, _) => (
                format!(r"*\\{marker}\\*.ps1"),
                format!(r"C:\Scripts\{marker}\deploy.ps1"),
                format!(r"C:\Scripts\{marker}\deploy.ps1.bak"),
                "suffix_and_interior",
            ),
            (Self::Commands, 0) => (
                format!("*powershell* -EncodedCommand {marker}*"),
                format!("powershell.exe -NoProfile -EncodedCommand {marker}=="),
                format!("powershell.exe -NoProfile -Command {marker}=="),
                "infix",
            ),
            (Self::Commands, 1) => (
                format!("*curl*https://{marker}.example/*"),
                format!("curl -s https://{marker}.example/payload"),
                format!("curl -s http://{marker}.example/payload"),
                "infix",
            ),
            (Self::Commands, _) => (
                format!("*/bin/tool-{marker} --mode ??"),
                format!("/opt/bin/tool-{marker} --mode ab"),
                format!("/opt/bin/tool-{marker} --mode abc"),
                "fixed_width",
            ),
        };
        Case {
            wildcard,
            positive,
            negative,
            shape,
        }
    }
}
