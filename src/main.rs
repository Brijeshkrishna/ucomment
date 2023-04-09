use async_recursion::async_recursion;
use reqwest::{header, Client};
use scraper::{Html, Selector};
use serde_json::{json, Value};

use csv::Writer;

#[inline]
fn render_comment(temp: &Value) -> (usize, String, String, String) {
    (
        temp.get("voteCount")
            .map(|x| {
                x["simpleText"]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<usize>()
                    .unwrap_or_default()
            })
            .unwrap_or_default(),
        temp["authorText"]["simpleText"]
            .as_str()
            .unwrap_or("")
            .to_string()
            .replace("@", ""),
        temp["contentText"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .fold(String::new(), |mut acc, x| {
                acc.push_str(x["text"].as_str().unwrap_or(""));
                acc
            }),
        temp["authorEndpoint"]["commandMetadata"]["webCommandMetadata"]["url"]
            .as_str()
            .unwrap()[9..]
            .to_string(),
    )
}

async fn get_token(client: &Client, url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let res = client.get(url).send().await?.text().await?;

    let document = Html::parse_document(&res);

    let token = document
        .select(&Selector::parse("script").unwrap())
        .find(|x| x.inner_html().contains("ytInitialData"))
        .map(|x| x.inner_html()[19..x.inner_html().len() - 1].to_string())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|data: _| {
            data["engagementPanels"].as_array().and_then(|panels| {
                panels.iter().find_map(|panel| {
                    let header = &panel["engagementPanelSectionListRenderer"]["header"]
                        ["engagementPanelTitleHeaderRenderer"]["menu"]["sortFilterSubMenuRenderer"];

                    let item = &panel["engagementPanelSectionListRenderer"]["content"]
                        ["sectionListRenderer"]["contents"][0]["itemSectionRenderer"]["contents"]
                        ["continuationItemRenderer"]["continuationEndpoint"];

                    header["subMenuItems"]
                        .as_array()
                        .and_then(|items| items.get(0))
                        .and_then(|item| {
                            item["serviceEndpoint"]["continuationCommand"]["token"].as_str()
                        })
                        .or_else(|| item["continuationCommand"]["token"].as_str())
                        .map(|endpoint| endpoint.to_string())
                })
            })
        })
        .ok_or_else(|| <&str as Into<&str>>::into("No token found in response"))?;
    Ok(token)
}

#[async_recursion]
async fn get_comment(
    client: &Client,
    cont: String,
    count: &mut i32,
    file: &mut Writer<std::fs::File>,
) -> Result<(), Box<dyn std::error::Error>> {
    let res: Value = client
        .post("https://www.youtube.com/youtubei/v1/next")
        .header(header::CONTENT_TYPE,"application/json".parse::<header::HeaderValue>().unwrap())
        .json(&json!({
        "context": {
            "client": {
                "userAgent": "Mozilla/5.0 (X11; Linux x86_64; rv:109.0) Gecko/20100101 Firefox/109.0,gzip(gfe)",
                "clientName": "WEB",
                "clientVersion": "2.20230120.00.00"
            }
        },
        "continuation": cont,
    }))
        .send()
        .await?
        .json()
        .await?;

    let users = res["onResponseReceivedEndpoints"][0]["appendContinuationItemsAction"]
        .get("continuationItems")
        .cloned()
        .unwrap_or_else(|| {
            res["onResponseReceivedEndpoints"][1]["reloadContinuationItemsCommand"]
                ["continuationItems"]
                .clone()
        });

    for user in users.as_array().unwrap_or(&vec![]) {
        let comment_thread_renderer = user.get("commentThreadRenderer");
        let comment_renderer = user.get("commentRenderer");
        let continuation_item_renderer = user.get("continuationItemRenderer");

        if let Some(comment_renderer) = comment_thread_renderer
            .and_then(|r| r.get("comment").and_then(|c| c.get("commentRenderer")))
        {
            let (votes, author, comment, id) = render_comment(comment_renderer);
            file.write_record(&[id, author, comment, format!("{votes}")]);
            // csv_data.push_str(format!("{},{},{}\n",votes, author, comment).as_str());
            //println!(
            //  "Author ➡ {}\nVotes ➡    {}\nComment ➡ {}\n",
            // author, votes, comment
            //);
            *count += 1;
        } else if let Some(comment_renderer) = comment_renderer {
            // let (votes, author, comment) = render_comment(comment_renderer);
            // csv_data.push_str(format!("{},{},{}\n",votes, author, comment).as_str());
            let (votes, author, comment, id) = render_comment(comment_renderer);

            file.write_record(&[id, author, comment, format!("{votes}")]);

            //println!(
            //  "\tAuthor ➡ {}\n\tVotes ➡    {}\n\tComment ➡ {}\n",
            // author, votes, comment
            //);
            *count += 1;
        } else if let Some(x) = continuation_item_renderer.and_then(|r| {
            r["button"]
                .get("buttonRenderer")
                .and_then(|br| br["command"]["continuationCommand"]["token"].as_str())
        }) {
            get_comment(client, x.to_string(), count, file).await?;
        } else if let Some(temp) = continuation_item_renderer.and_then(|r| {
            r.get("continuationEndpoint")
                .and_then(|ce| ce["continuationCommand"]["token"].as_str())
        }) {
            get_comment(client, temp.to_string(), count, file).await?;
        }

        if let Some(reply_renderer) = comment_thread_renderer.and_then(|r| r.get("replies")) {
            let temp = reply_renderer["commentRepliesRenderer"]["contents"][0]
                ["continuationItemRenderer"]["continuationEndpoint"]["continuationCommand"]
                ["token"]
                .as_str()
                .unwrap();
            get_comment(client, temp.to_string(), count, file).await?;
        }
        print!("{count}\r");
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {

    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        println!("Usage: ucomment <video id>");
        return Ok(());
    }

    let id:&str = &args[1];


    let mut count: i32 = 0;

    let client = Client::new();
    let mut wtr = Writer::from_path(format!("{id}.csv")).unwrap();
    wtr.write_record(&["UserId", "Author", "Comment", "Likes"]);
    get_comment(
        &client,
        get_token(&client, format!("https://www.youtube.com/watch?v={id}").as_str()).await?,
        &mut count,
        &mut wtr,
    )
    .await?;
    println!("total = {}", count);
    wtr.flush().unwrap();
    
    Ok(())

}


