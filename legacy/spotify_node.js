#!/usr/bin/env node
require("dotenv").config();

const SpotifyWebApi = require("spotify-web-api-node");
const express = require("express");
const fs = require("fs-extra");
const path = require("path");
const yargs = require("yargs/yargs");
const { hideBin } = require("yargs/helpers");
const youtubeDl = require("youtube-dl-exec");
const sanitize = require("sanitize-filename");
const player = require("play-sound")((opts = {}));
const colors = require("colors");
const axios = require("axios");
const NodeID3 = require("node-id3");

const openImport = require("open");
const open = openImport.default || openImport;
const boxenImport = require("boxen");
const boxen = boxenImport.default || boxenImport;
const oraImport = require("ora");
const ora = oraImport.default || oraImport;
const ConfImport = require("conf");
const Conf = ConfImport.default || ConfImport;
const inquirerImport = require("inquirer");
const inquirer = inquirerImport.default || inquirerImport;
const figletImport = require("figlet");
const figlet = figletImport.default || figletImport;

const CACHE_DIR = path.join(__dirname, "cache");
const TOKEN_FILE = path.join(__dirname, ".spotify_refresh_token.secret.json");
const ICON_PATH = path.join(__dirname, "Data", "icon.png");
const SOUND_INFO_PATH = path.join(__dirname, "Data", "i.mp3");
const SOUND_WARN_PATH = path.join(__dirname, "Data", "w.mp3");
const CONFIG_SCHEMA = {
  downloadPath: {
    type: "string",
    default: path.join(__dirname, "Downloads"),
  },
};
const REDIRECT_URI =
  process.env.SPOTIFY_REDIRECT_URI || "http://localhost:5000/callback";
const PORT = 5000;
const SCOPES = [
  "user-library-read",
  "playlist-read-private",
  "playlist-read-collaborative",
];
const YT_DLP_OPTIONS = {
  output: "%(id)s.%(ext)s",
  format: "bestaudio/best",
  extractAudio: true,
  audioFormat: "mp3",
  audioQuality: 0,

  noPlaylist: true,
  noWarning: true,
};

class ConfigManager {
  constructor() {
    this.conf = new Conf({
      schema: CONFIG_SCHEMA,
      projectName: "spotify-dl-node",
    });
  }
  get(key) {
    return this.conf.get(key);
  }
  set(key, value) {
    this.conf.set(key, value);
  }
  getDownloadPath() {
    return path.resolve(this.get("downloadPath"));
  }
  async setDownloadPath(newPath) {
    const absolutePath = path.resolve(newPath);
    try {
      await fs.ensureDir(path.dirname(absolutePath));
      this.set("downloadPath", absolutePath);
      console.log(colors.green(`Download path set to: ${absolutePath}`));
      return true;
    } catch (error) {
      console.error(
        colors.red(
          `Error setting download path: Could not ensure parent directory exists for "${absolutePath}"`
        ),
        error
      );
      return false;
    }
  }
  getConfigPath() {
    return this.conf.path;
  }
}
const configManager = new ConfigManager();

async function readJsonFile(filePath) {
  try {
    await fs.ensureFile(filePath);
    const data = await fs.readFile(filePath, "utf8");
    return data ? JSON.parse(data) : {};
  } catch (error) {
    if (error.code === "ENOENT") return {};
    console.error(colors.red(`Error reading JSON file ${filePath}:`), error);
    throw error;
  }
}
async function writeJsonFile(filePath, data) {
  try {
    await fs.writeJson(filePath, data, { spaces: 2 });
  } catch (error) {
    console.error(colors.red(`Error writing JSON file ${filePath}:`), error);
    throw error;
  }
}
function parseSpotifyUri(input) {
  try {
    if (!input || typeof input !== "string")
      return { type: null, id: null, error: "Invalid input provided." };
    input = input.trim();
    if (input.includes("open.spotify.com")) {
      const url = new URL(input);
      const pathParts = url.pathname.split("/").filter((p) => p);
      if (
        pathParts.length >= 2 &&
        ["track", "album", "playlist"].includes(pathParts[0])
      )
        return { type: pathParts[0], id: pathParts[1] };
    } else if (input.startsWith("spotify:")) {
      const parts = input.split(":");
      if (
        parts.length === 3 &&
        ["track", "album", "playlist"].includes(parts[1])
      )
        return { type: parts[1], id: parts[2] };
    } else if (/^[a-zA-Z0-9]{22}$/.test(input)) {
      return { type: "id_only", id: input };
    }
    return {
      type: null,
      id: null,
      error: `Could not parse input: "${input}". Please provide a valid Spotify URL, URI, or ID.`,
    };
  } catch (e) {
    return {
      type: null,
      id: null,
      error: `Error parsing input "${input}": ${e.message}`,
    };
  }
}

class Notify {
  static _bannerContent = null;
  static _redrawCallback = null;

  static setBanner(bannerContent) {
    Notify._bannerContent = bannerContent;
  }

  static setRedrawCallback(callback) {
    Notify._redrawCallback = callback;
  }

  static redraw() {
    if (Notify._redrawCallback) {
      Notify._redrawCallback();
    }
  }

  static async send(
    message,
    thumbnailImg,
    { error = false, downloaded = true, title = null } = {}
  ) {
    const defaultTitle = downloaded ? "Downloaded" : "Start downloading";
    const soundPath = error ? SOUND_WARN_PATH : SOUND_INFO_PATH;
    const finalTitle = title || defaultTitle;

    // Determine toast style based on type
    let borderColor = "cyan";
    let icon = "✓";
    if (error) {
      borderColor = "red";
      icon = "✗";
    } else if (downloaded) {
      borderColor = "green";
      icon = "✓";
    } else {
      borderColor = "yellow";
      icon = "→";
    }

    const toastContent = `${colors[borderColor](icon)} ${colors.bold(
      finalTitle
    )}\n${colors.grey(message)}`;

    const toastBox = boxen(toastContent, {
      padding: { top: 0, bottom: 0, left: 1, right: 1 },
      margin: { top: 1, bottom: 0, left: 0, right: 0 },
      borderStyle: "round",
      borderColor: borderColor,
      title: finalTitle,
      titleAlignment: "left",
    });

    // Get terminal width and toast dimensions
    const terminalWidth = process.stdout.columns || 80;
    const toastLines = toastBox.split("\n");
    const toastWidth = Math.max(
      ...toastLines.map((line) => {
        // Remove ANSI codes to get actual width
        return line.replace(/\x1b\[[0-9;]*m/g, "").length;
      })
    );

    // Calculate position for top right corner
    const rightMargin = 2; // Space from right edge
    const leftPadding = Math.max(0, terminalWidth - toastWidth - rightMargin);

    // Use row 3 to avoid overwriting the banner (row 1) and leave some space
    const toastStartRow = 3;

    // Save cursor position before showing toast
    process.stdout.write("\x1b[s");

    // Position cursor at top right (fixed row, calculated column)
    process.stdout.write(`\x1b[${toastStartRow};${leftPadding + 1}H`);

    // Print each line of the toast
    toastLines.forEach((line, index) => {
      if (index > 0) {
        // Move to start of next line
        process.stdout.write(
          `\x1b[${toastStartRow + index};${leftPadding + 1}H`
        );
      }
      process.stdout.write(line);
      // Clear the rest of the line to avoid artifacts
      process.stdout.write("\x1b[K");
    });

    // Restore cursor to original position so normal output continues
    process.stdout.write("\x1b[u");

    // Clear toast after 3 seconds and redraw everything
    const clearTimeout = setTimeout(() => {
      try {
        // Clear the screen and redraw everything
        console.clear();
        Notify.redraw();
      } catch (err) {
        // Silently fail if clearing doesn't work (e.g., if output is redirected)
      }
    }, 3000);

    // Store timeout reference in case we need to clear it manually
    Notify._lastToastTimeout = clearTimeout;

    // Play sound if available
    if (await fs.pathExists(soundPath)) {
      player.play(soundPath, (err) => {
        if (err)
          console.error(colors.yellow("Warning: Error playing sound:"), err);
      });
    }
  }
}

class TagEditor {
  async setTags(filePath, tags) {
    const spinner = ora(
      colors.cyan(`Tagging ${path.basename(filePath)}...`)
    ).start();
    try {
      // Check if file exists
      if (!(await fs.pathExists(filePath))) {
        throw new Error(`File does not exist: ${filePath}`);
      }

      // Build ID3 tags object
      const id3Tags = {};

      if (tags.name) {
        id3Tags.title = String(tags.name);
      }
      if (tags.artist && Array.isArray(tags.artist) && tags.artist.length > 0) {
        id3Tags.artist = tags.artist.join("/");
      }
      if (tags.album) {
        id3Tags.album = String(tags.album);
      }
      if (tags.year) {
        id3Tags.year = String(tags.year);
      }

      // Handle album art
      if (tags.image) {
        spinner.text = colors.cyan(
          `Tagging ${path.basename(filePath)} (Downloading Art)...`
        );
        try {
          const response = await axios.get(tags.image, {
            responseType: "arraybuffer",
            timeout: 10000,
          });
          const imageBuffer = Buffer.from(response.data, "binary");
          let mimeType = "image/jpeg";
          if (tags.image.includes(".png")) mimeType = "image/png";
          id3Tags.image = {
            mime: mimeType,
            type: { id: 3, description: "Cover (front)" },
            description: "Album Art",
            imageBuffer: imageBuffer,
          };
        } catch (imgError) {
          spinner.warn(
            colors.yellow(
              `Warning: Failed to download or add album art for ${tags.name}: ${imgError.message}`
            )
          );
        }
      }

      spinner.text = colors.cyan(
        `Tagging ${path.basename(filePath)} (Writing Tags)...`
      );

      // Write tags using NodeID3
      try {
        const result = await NodeID3.Promise.write(id3Tags, filePath);
        if (result === true) {
          spinner.succeed(
            colors.green(`Successfully tagged ${path.basename(filePath)}`)
          );
        } else {
          // If write returns false, try to get more info
          throw new Error(
            "NodeID3.write returned false - file may be locked or invalid"
          );
        }
      } catch (writeError) {
        // Check if file is locked or has permission issues
        try {
          await fs.access(filePath, fs.constants.W_OK);
        } catch (accessError) {
          throw new Error(
            `Permission denied or file locked: ${accessError.message}`
          );
        }
        throw writeError;
      }
    } catch (error) {
      spinner.fail(
        colors.red(
          `Failed to write tags to ${path.basename(filePath)}: ${
            error.message || error
          }`
        )
      );
    }
  }
}

class YoutubeDownloader {
  async findTrack(query, durationMs) {
    const spinner = ora(
      colors.cyan(`Searching YouTube for "${query}"...`)
    ).start();
    try {
      const searchOptions = {
        ...YT_DLP_OPTIONS,
        dumpSingleJson: true,

        matchFilter: durationMs
          ? `duration > ${durationMs / 1000 - 30} & duration < ${
              durationMs / 1000 + 30
            }`
          : null,
        defaultSearch: "ytsearch1:",

        extractAudio: undefined,
        audioFormat: undefined,
        audioQuality: undefined,
        output: undefined,
      };

      if (!searchOptions.matchFilter) delete searchOptions.matchFilter;

      const output = await youtubeDl(query, searchOptions);
      if (output && output.webpage_url) {
        spinner.succeed(colors.green(`Found YouTube match: ${output.title}`));
        return { url: output.webpage_url, title: output.title };
      }
      spinner.warn(
        colors.yellow(`No suitable YouTube match found for "${query}".`)
      );
      return null;
    } catch (error) {
      spinner.fail(colors.red(`YouTube search failed for "${query}".`));

      return null;
    }
  }

  async downloadAndConvert(youtubeUrl, outputFilename) {
    const outputDir = path.dirname(outputFilename);
    const baseName = path.basename(outputFilename, ".mp3");

    const tempOutputPath = path.join(outputDir, `${baseName}.%(ext)s`);
    const finalOutputPath = outputFilename;

    const spinner = ora(
      colors.cyan(
        `Downloading & Converting: ${path.basename(finalOutputPath)}...`
      )
    ).start();
    await fs.ensureDir(outputDir);

    try {
      const downloadOptions = {
        ...YT_DLP_OPTIONS,
        output: tempOutputPath,
      };
      await youtubeDl(youtubeUrl, downloadOptions);
      spinner.succeed(
        colors.green(
          `Downloaded & Converted: ${path.basename(finalOutputPath)}`
        )
      );
      return true;
    } catch (error) {
      spinner.fail(
        colors.red(
          `Download/Conversion failed: ${path.basename(finalOutputPath)}`
        )
      );
      console.error(
        colors.grey(`yt-dlp error: ${error.shortMessage || error.message}`)
      );

      try {
        const files = await fs.readdir(outputDir);
        for (const file of files) {
          if (file.startsWith(baseName)) {
            await fs.remove(path.join(outputDir, file));
          }
        }
      } catch (cleanupError) {
        console.warn(
          colors.yellow("Warning: Error during cache cleanup:"),
          cleanupError
        );
      }
      return false;
    }
  }

  async getNameFromYoutube(url) {
    const spinner = ora(
      colors.cyan(`Getting title for YouTube URL...`)
    ).start();
    try {
      const output = await youtubeDl(url, {
        getTitle: true,
        skipDownload: true,
        ...YT_DLP_OPTIONS,
      });
      const title = output?.trim();
      if (title) {
        spinner.succeed(colors.green(`Got YouTube title: ${title}`));
        return title;
      }
      spinner.warn(colors.yellow(`Could not extract title from YouTube URL.`));
      return "Unknown YouTube Video";
    } catch (error) {
      spinner.fail(colors.red(`Failed to get title from YouTube URL.`));

      return "Unknown YouTube Video";
    }
  }
}

class Spotify {
  constructor(credentials) {
    this.clientId = credentials.clientId;
    this.clientSecret = credentials.clientSecret;
    this.spotifyApi = new SpotifyWebApi({
      clientId: this.clientId,
      clientSecret: this.clientSecret,
    });
    this.accessToken = null;
  }
  async initialize() {
    try {
      const data = await this.spotifyApi.clientCredentialsGrant();
      this.accessToken = data.body["access_token"];
      this.spotifyApi.setAccessToken(this.accessToken);
    } catch (error) {
      throw error;
    }
  }
  async getSongInfo(trackId) {
    if (!this.accessToken) throw new Error("Spotify client not initialized.");
    try {
      const { body: track } = await this.spotifyApi.getTrack(trackId);
      return {
        uri: track.uri.split(":").pop(),
        name: track.name,
        artist: track.artists.map((a) => a.name),
        album: track.album.name,
        image: track.album.images?.[0]?.url,
        duration_ms: track.duration_ms,
        year: track.album.release_date
          ? track.album.release_date.substring(0, 4)
          : null,
      };
    } catch (error) {
      console.error(
        colors.red(`Error getting track info for ${trackId}:`),
        error.body?.error?.message || error.message
      );
      if (error.statusCode === 401)
        console.error(colors.yellow("Token likely expired."));
      throw error;
    }
  }
  async getAlbum(albumId) {
    if (!this.accessToken) throw new Error("Spotify client not initialized.");
    try {
      const { body: album } = await this.spotifyApi.getAlbum(albumId);
      const albumYear = album.release_date
        ? album.release_date.substring(0, 4)
        : null;
      let allTracks = [];
      let offset = 0;
      const limit = 50;
      let trackResponse;
      do {
        trackResponse = await this.spotifyApi.getAlbumTracks(albumId, {
          limit,
          offset,
        });
        const tracks = trackResponse.body.items.map((track) => ({
          uri: track.uri.split(":").pop(),
          name: track.name,
          artist: track.artists.map((a) => a.name),
          album: album.name,
          image: album.images?.[0]?.url,
          duration_ms: track.duration_ms,
          year: albumYear,
          preview_url: track.preview_url,
        }));
        allTracks = allTracks.concat(tracks);
        offset += limit;
      } while (trackResponse.body.next);
      return {
        name: album.name,
        artist: album.artists.map((a) => a.name).join(", "),
        copyright: album.copyrights?.[0]?.text,
        image: album.images?.[0]?.url,
        year: albumYear,
        tracks: allTracks,
      };
    } catch (error) {
      console.error(
        colors.red(`Error getting album ${albumId}:`),
        error.body?.error?.message || error.message
      );
      if (error.statusCode === 401)
        console.error(colors.yellow("Token likely expired."));
      return null;
    }
  }
  async search(query, types = ["track"], limit = 1) {
    if (!this.accessToken) throw new Error("Spotify client not initialized.");
    try {
      const data = await this.spotifyApi.search(query, types, { limit });
      if (types.includes("track") && data.body.tracks?.items?.length > 0) {
        const track = data.body.tracks.items[0];
        return {
          uri: track.uri.split(":").pop(),
          name: track.name,
          artist: track.artists.map((a) => a.name),
          album: track.album.name,
          image: track.album.images?.[0]?.url,
          duration_ms: track.duration_ms,
          year: track.album.release_date
            ? track.album.release_date.substring(0, 4)
            : null,
        };
      }
      return false;
    } catch (error) {
      console.error(
        colors.red(`Error searching for "${query}":`),
        error.body?.error?.message || error.message
      );
      if (error.statusCode === 401)
        console.error(colors.yellow("Token likely expired."));
      throw error;
    }
  }
  async getDuration(trackIdOrUri) {
    if (!this.accessToken) throw new Error("Spotify client not initialized.");
    const trackId = trackIdOrUri.includes(":")
      ? trackIdOrUri.split(":").pop()
      : trackIdOrUri;
    try {
      const data = await this.spotifyApi.getTrack(trackId);
      return data.body.duration_ms;
    } catch (error) {
      console.error(
        colors.red(`Error getting duration for ${trackId}:`),
        error.body?.error?.message || error.message
      );
      if (error.statusCode === 401)
        console.error(colors.yellow("Token likely expired."));
      throw error;
    }
  }
}
class SpotifyUser {
  constructor(credentials) {
    this.clientId = credentials.clientId;
    this.clientSecret = credentials.clientSecret;
    this.redirectUri = REDIRECT_URI;
    this.scopes = SCOPES;
    this.spotifyApi = new SpotifyWebApi({
      clientId: this.clientId,
      clientSecret: this.clientSecret,
      redirectUri: this.redirectUri,
    });
    this.refreshTokenData = null;
  }
  async initialize() {
    try {
      this.refreshTokenData = await readJsonFile(TOKEN_FILE);
      if (this.refreshTokenData?.refresh_token) {
        this.spotifyApi.setRefreshToken(this.refreshTokenData.refresh_token);
        const data = await this.spotifyApi.refreshAccessToken();
        this.spotifyApi.setAccessToken(data.body["access_token"]);
        if (data.body["refresh_token"]) {
          this.refreshTokenData.refresh_token = data.body["refresh_token"];
          this.spotifyApi.setRefreshToken(this.refreshTokenData.refresh_token);
          await writeJsonFile(TOKEN_FILE, this.refreshTokenData);
        }
      } else {
        await this.performAuthorizationFlow();
      }
    } catch (error) {
      if (
        error.body &&
        (error.body.error === "invalid_grant" ||
          error.body.error === "invalid_client")
      ) {
        console.warn(
          colors.yellow(
            `Refresh token invalid/revoked (${error.body.error}). Starting full authorization flow.`
          )
        );
        await this.performAuthorizationFlow();
      } else {
        console.error(
          colors.red("Error during refresh token check or refresh:"),
          error.message || error
        );
        console.log(colors.yellow("Falling back to full authorization flow."));
        await this.performAuthorizationFlow();
      }
    }
    return this.spotifyApi;
  }
  async performAuthorizationFlow() {
    return new Promise(async (resolve, reject) => {
      const app = express();
      let server;
      const state = Math.random().toString(36).substring(7);
      const authorizeURL = this.spotifyApi.createAuthorizeURL(
        this.scopes,
        state
      );
      app.get("/callback", async (req, res) => {
        const { code, state: returnedState, error } = req.query;
        const closeServer = () => {
          if (server) {
            server.close(() => {
              console.log(colors.grey("Callback server closed."));
              server = null;
            });
          }
        };
        if (error) {
          console.error(colors.red("Callback Error:"), error);
          res
            .status(400)
            .send(`Callback Error: ${error}. Please close window.`);
          reject(new Error(`Callback Error: ${error}`));
          closeServer();
          return;
        }
        if (returnedState !== state) {
          console.error(colors.red("State mismatch error."));
          res.status(400).send("State mismatch error. Please close window.");
          reject(new Error("State mismatch error."));
          closeServer();
          return;
        }
        res.send("Authorization successful! You can close this window now.");
        console.log(
          colors.green("Authorization code received. Exchanging for tokens...")
        );
        try {
          const data = await this.spotifyApi.authorizationCodeGrant(code);
          const accessToken = data.body["access_token"];
          const refreshToken = data.body["refresh_token"];
          this.spotifyApi.setAccessToken(accessToken);
          this.spotifyApi.setRefreshToken(refreshToken);
          console.log(colors.green("Tokens obtained successfully."));
          await writeJsonFile(TOKEN_FILE, { refresh_token: refreshToken });
          console.log(colors.grey("Refresh token saved."));
          resolve();
        } catch (grantError) {
          console.error(
            colors.red("Error getting Tokens:"),
            grantError.body || grantError.message
          );
          reject(new Error(`Token Grant Error: ${grantError.message}`));
        } finally {
          closeServer();
        }
      });
      server = app.listen(PORT, async () => {
        console.log(
          colors.yellow(`Temporary callback server running on http:`)
        );
        console.log(
          colors.yellow("Opening browser for Spotify authorization...")
        );
        console.log(
          colors.grey("If the browser does not open, please manually visit:")
        );
        console.log(colors.grey(authorizeURL));
        try {
          await open(authorizeURL);
        } catch (openError) {
          console.warn(
            colors.yellow("Could not automatically open browser:"),
            openError.message
          );
        }
      });
      server.on("error", (err) => {
        console.error(colors.red("Server Error:"), err);
        if (err.code === "EADDRINUSE") {
          console.error(colors.red(`Port ${PORT} is already in use.`));
        }
        reject(err);
        if (server) server.close();
      });
    });
  }
  async getPlaylistTracks(playlistId) {
    if (!this.spotifyApi.getAccessToken())
      throw new Error(
        "User API client not initialized. Run initialize() first."
      );
    const spinner = ora(
      colors.cyan(`Fetching playlist tracks for ${playlistId}...`)
    ).start();
    try {
      let allTracks = [];
      let offset = 0;
      const limit = 50;
      let response;
      do {
        response = await this.spotifyApi.getPlaylistTracks(playlistId, {
          limit: limit,
          offset: offset,
          fields:
            "items(track(name,uri,artists(name),album(name,images),duration_ms)),total,next",
        });
        const tracks = response.body.items
          .map((item) => {
            if (!item.track || !item.track.uri) return null;
            return {
              uri: item.track.uri.split(":").pop(),
              name: item.track.name,
              artist: item.track.artists.map((artist) => artist.name),
              album: item.track.album.name,
              image:
                item.track.album.images.length > 0
                  ? item.track.album.images[0].url
                  : null,
              duration_ms: item.track.duration_ms,
              year: null,
            };
          })
          .filter((track) => track !== null);
        allTracks = allTracks.concat(tracks);
        offset += limit;
      } while (response.body.next);
      spinner.succeed(colors.green(`Fetched ${allTracks.length} tracks.`));
      return allTracks;
    } catch (error) {
      spinner.fail(
        colors.red(
          `Failed to fetch playlist tracks: ${
            error.body?.error?.message || error.message
          }`
        )
      );
      throw error;
    }
  }
}

class MusicDownloader {
  constructor(spotifyClient, spotifyUserClient, configMgr) {
    this.spotify = spotifyClient;
    this.spotifyUser = spotifyUserClient;
    this.config = configMgr;
    this.youtube = new YoutubeDownloader();
    this.tagEditor = new TagEditor();
  }
  _getSafeFilename(info, targetDir) {
    const baseName = sanitize(`${info.artist[0]} - ${info.name}`);
    return path.join(targetDir, `${baseName}.mp3`);
  }
  async _processTrack(info, targetPathOverride) {
    if (!info || !info.uri) {
      console.error(colors.red("[Process] Invalid track info received."));
      return false;
    }
    const trackName = `${info.artist[0]} - ${info.name}`;
    const cacheTrackDir = path.join(CACHE_DIR, info.uri);
    const tempFilePath = path.join(cacheTrackDir, `${info.uri}.mp3`);
    const finalDir = targetPathOverride || this.config.getDownloadPath();
    const finalFilePath = this._getSafeFilename(info, finalDir);
    await fs.ensureDir(cacheTrackDir);
    await fs.ensureDir(finalDir);
    await Notify.send(trackName, {
      downloaded: false,
      title: "Starting Download",
    });
    const overallSpinner = ora(colors.cyan(`Processing: ${trackName}`)).start();
    try {
      const ytQuery = trackName.replace(/['",.\/]/g, "");
      overallSpinner.text = colors.cyan(
        `Processing: ${trackName} (Finding on YouTube)`
      );
      const youtubeResult = await this.youtube.findTrack(
        ytQuery,
        info.duration_ms
      );
      if (!youtubeResult?.url) {
        await Notify.send(`Could not find "${trackName}" on YouTube`, {
          error: true,
          title: "Download Failed",
        });
        throw new Error("YouTube search failed");
      }
      overallSpinner.text = colors.cyan(
        `Processing: ${trackName} (Downloading)`
      );
      const downloadSuccess = await this.youtube.downloadAndConvert(
        youtubeResult.url,
        tempFilePath
      );
      if (!downloadSuccess) {
        await Notify.send(
          `Failed to download/convert "${trackName}" from YouTube`,
          { error: true, title: "Download Failed" }
        );
        throw new Error("YouTube download/conversion failed");
      }
      overallSpinner.text = colors.cyan(
        `Processing: ${trackName} (Finalizing - Moving File)`
      );
      await fs.move(tempFilePath, finalFilePath, { overwrite: true });

      // Wait a bit to ensure file is fully written and not locked
      await new Promise((resolve) => setTimeout(resolve, 500));

      overallSpinner.text = colors.cyan(`Processing: ${trackName} (Tagging)`);
      await this.tagEditor.setTags(finalFilePath, info);

      await Notify.send(trackName, {
        downloaded: true,
        title: "Download Complete",
        thumbnailImg: info.image,
      });
      overallSpinner.succeed(
        colors.green(`Successfully processed: ${trackName} and tagged.`)
      );
      return true;
    } catch (error) {
      overallSpinner.fail(
        colors.red(`Failed processing: ${trackName} - ${error.message}`)
      );
      return false;
    } finally {
      try {
        await fs.remove(cacheTrackDir);
      } catch (cleanupError) {}
    }
  }
  async downloadBySpotifyUri(inputUri, targetPath) {
    const { type, id, error } = parseSpotifyUri(inputUri);
    if (error || !id || (type !== "track" && type !== "id_only")) {
      console.error(
        colors.red(
          `Invalid Spotify Track input: ${
            error || "Must be a track URL/URI/ID."
          }`
        )
      );
      return false;
    }
    try {
      const info = await this.spotify.getSongInfo(id);
      if (info) return await this._processTrack(info, targetPath);
    } catch (error) {
      if (error.statusCode === 401 || error.message?.includes("initialized")) {
        console.warn(
          colors.yellow(
            `Client credentials failed for track ${id}, trying user auth...`
          )
        );
        try {
          await this.spotifyUser.initialize();
          const { body: track } = await this.spotifyUser.spotifyApi.getTrack(
            id
          );
          if (track) {
            const formattedInfo = {
              uri: track.uri.split(":").pop(),
              name: track.name,
              artist: track.artists.map((a) => a.name),
              album: track.album.name,
              image: track.album.images?.[0]?.url,
              duration_ms: track.duration_ms,
              year: track.album.release_date
                ? track.album.release_date.substring(0, 4)
                : null,
            };
            return await this._processTrack(formattedInfo, targetPath);
          }
        } catch (userError) {
          console.error(
            colors.red(`User auth also failed for track ${id}.`),
            userError.body || userError.message
          );
        }
      } else {
        console.error(
          colors.red(`Error getting track ${id} info:`),
          error.body || error.message
        );
      }
    }
    await Notify.send(`Failed to get info for Spotify track: ${id}`, {
      error: true,
    });
    return false;
  }
  async downloadBySpotifyUriAlbumMode(inputUri, targetPath) {
    const { type, id, error } = parseSpotifyUri(inputUri);
    if (error || !id || (type !== "album" && type !== "id_only")) {
      console.error(
        colors.red(
          `Invalid Spotify Album input: ${
            error || "Must be an album URL/URI/ID."
          }`
        )
      );
      return false;
    }
    const spinner = ora(
      colors.cyan(`Fetching album tracks for ${id}...`)
    ).start();
    try {
      const albumData = await this.spotify.getAlbum(id);
      if (!albumData?.tracks) {
        spinner.fail(colors.red(`Could not find album or tracks for: ${id}`));
        return false;
      }
      spinner.succeed(
        colors.green(
          `Found ${albumData.tracks.length} tracks in album: ${albumData.name}`
        )
      );
      let successCount = 0;
      for (let i = 0; i < albumData.tracks.length; i++) {
        const trackInfo = albumData.tracks[i];
        console.log(
          colors.magenta(
            `\n[${i + 1}/${albumData.tracks.length}]--------------------`
          )
        );
        try {
          const success = await this._processTrack(trackInfo, targetPath);
          if (success) successCount++;
        } catch (error) {
          console.error(
            colors.red(
              `Error processing album track ${i + 1}: ${error.message}`
            )
          );
        }
      }
      console.log(
        colors.magenta(
          `\n--------------------\nFinished album download. ${successCount}/${albumData.tracks.length} tracks successful.`
        )
      );
      return successCount === albumData.tracks.length;
    } catch (error) {
      spinner.fail(
        colors.red(
          `Failed to get or process album ${id}: ${error.body || error.message}`
        )
      );
      return false;
    }
  }
  async downloadBySpotifyUriPlaylistMode(inputUri, targetPath) {
    const { type, id, error } = parseSpotifyUri(inputUri);
    if (error || !id || (type !== "playlist" && type !== "id_only")) {
      console.error(
        colors.red(
          `Invalid Spotify Playlist input: ${
            error || "Must be a playlist URL/URI/ID."
          }`
        )
      );
      return false;
    }
    try {
      await this.spotifyUser.initialize();
      const tracks = await this.spotifyUser.getPlaylistTracks(id);
      let successCount = 0;
      for (let i = 0; i < tracks.length; i++) {
        const trackInfo = tracks[i];
        console.log(
          colors.magenta(`\n[${i + 1}/${tracks.length}]--------------------`)
        );
        try {
          const success = await this._processTrack(trackInfo, targetPath);
          if (success) successCount++;
        } catch (error) {
          console.error(
            colors.red(
              `Error processing playlist track ${i + 1}: ${error.message}`
            )
          );
        }
      }
      console.log(
        colors.magenta(
          `\n--------------------\nFinished playlist download. ${successCount}/${tracks.length} tracks successful.`
        )
      );
      return successCount === tracks.length;
    } catch (error) {
      console.error(
        colors.red(
          `Failed to get or process playlist ${id}: ${
            error.body || error.message
          }`
        )
      );
      return false;
    }
  }
  async downloadBySearchQuery(query, targetPath) {
    const spinner = ora(colors.cyan(`Searching for "${query}"...`)).start();
    try {
      let info = await this.spotify.search(query);

      if (info) {
        spinner.succeed(
          colors.green(`Found match: ${info.artist[0]} - ${info.name}`)
        );
        return await this._processTrack(info, targetPath);
      } else {
        spinner.fail(
          colors.yellow(`Could not find any match for query: "${query}"`)
        );
        return false;
      }
    } catch (error) {
      spinner.fail(
        colors.red(`Error during search for "${query}": ${error.message}`)
      );
      return false;
    }
  }
  async downloadBySpotifyUriList(uris, targetPath) {
    let successCount = 0;
    console.log(
      colors.blue(`\nStarting download for ${uris.length} tracks from list...`)
    );
    for (let i = 0; i < uris.length; i++) {
      const uri = uris[i];
      console.log(
        colors.magenta(
          `\n[${i + 1}/${uris.length}] Processing URI: ${uri} -------------`
        )
      );
      try {
        const { type, id, error } = parseSpotifyUri(uri);
        if (error || !id || (type !== "track" && type !== "id_only")) {
          console.error(
            colors.yellow(
              `Skipping invalid input from file: "${uri}" - ${
                error || "Not a track."
              }`
            )
          );
          continue;
        }

        const success = await this.downloadBySpotifyUri(uri, targetPath);
        if (success) successCount++;
      } catch (error) {
        console.error(
          colors.red(`Error processing URI ${uri} from list:`),
          error.message
        );
        await Notify.send(`Failed to download track from URI: ${uri}`, {
          error: true,
        });
      }
    }
    console.log(
      colors.blue(
        `\n--------------------\nFinished list download. ${successCount}/${uris.length} tracks downloaded successfully.`
      )
    );
    return successCount === uris.length;
  }
}

async function runInteractiveMenu(md) {
  let exit = false;
  while (!exit) {
    console.log();
    const { action } = await inquirer.prompt([
      {
        type: 'list',
        name: 'action',
        message: 'What would you like to do?',
        choices: [
          { name: 'Download Song (URL/URI/ID)', value: 'song' },
          { name: 'Download Album (URL/URI/ID)', value: 'album' },
          { name: 'Download Playlist (URL/URI/ID)', value: 'playlist' },
          { name: 'Search and Download Track', value: 'query' },
          { name: 'Download from File (list of URIs)', value: 'file' },
          { name: 'Settings', value: 'settings' },
          { name: 'Exit', value: 'exit' }
        ]
      }
    ]);

    if (action === 'exit') {
      exit = true;
      console.log(colors.cyan('Goodbye!'));
      break;
    }

    if (action === 'settings') {
      console.log(colors.underline("\nCurrent Settings:"));
      console.log(` - Download Path: ${colors.cyan(configManager.getDownloadPath())}`);
      console.log(` - Config File: ${colors.grey(configManager.getConfigPath())}`);
      const answers = await inquirer.prompt([
        {
          type: "input",
          name: "newPath",
          message: "Enter new download path (leave blank to keep current):",
          default: configManager.getDownloadPath(),
          filter: (input) => input.trim(),
          validate: async (input) => {
            if (!input) return true;
            const absolutePath = path.resolve(input);
            try {
              await fs.access(path.dirname(absolutePath), fs.constants.W_OK);
              return true;
            } catch (err) {
              if (err.code === "ENOENT")
                return `Parent directory does not exist: ${path.dirname(absolutePath)}`;
              if (err.code === "EACCES")
                return `Permission denied for parent: ${path.dirname(absolutePath)}`;
              return `Invalid path: ${err.message}`;
            }
          },
        },
      ]);
      if (answers.newPath && answers.newPath !== configManager.getDownloadPath()) {
        await configManager.setDownloadPath(answers.newPath);
      } else {
        console.log(colors.grey("Download path unchanged."));
      }
      continue;
    }

    const prompts = {
      song: 'Enter Spotify track URL, URI, or ID:',
      album: 'Enter Spotify album URL, URI, or ID:',
      playlist: 'Enter Spotify playlist URL, URI, or ID:',
      query: 'Enter "Artist - Title" or search query:',
      file: 'Enter path to the file:'
    };

    const { input } = await inquirer.prompt([
      {
        type: 'input',
        name: 'input',
        message: prompts[action],
        validate: val => val.trim().length > 0 || 'Please provide an input.'
      }
    ]);

    console.log();
    try {
      switch (action) {
        case 'song':
          await md.downloadBySpotifyUri(input.trim());
          break;
        case 'album':
          await md.downloadBySpotifyUriAlbumMode(input.trim());
          break;
        case 'playlist':
          await md.downloadBySpotifyUriPlaylistMode(input.trim());
          break;
        case 'query':
          await md.downloadBySearchQuery(input.trim());
          break;
        case 'file':
          const filePath = path.resolve(input.trim());
          console.log(colors.blue(`Reading URIs from file: ${filePath}`));
          try {
            const fileContent = await fs.readFile(filePath, "utf8");
            const uris = fileContent
              .split(/[\r\n]+/)
              .map((line) => line.trim())
              .filter((line) => line.length > 0 && !line.startsWith("#"));
            if (uris.length > 0) {
              await md.downloadBySpotifyUriList(uris);
            } else {
              console.warn(colors.yellow(`File "${filePath}" is empty or contains no valid URIs.`));
            }
          } catch (fileError) {
            console.error(colors.red(`Error reading file "${filePath}":`), fileError.message);
          }
          break;
      }
    } catch (e) {
      console.error(colors.red(`An error occurred: ${e.message}`));
    }
  }
}

// Function to redraw the banner
function redrawBanner() {
  try {
    const asciiArt = figlet.textSync("Spotify DL", { font: "Standard" });
    const banner = boxen(colors.cyan(asciiArt), {
      padding: 1,
      margin: 1,
      borderStyle: "round",
      borderColor: "cyan",
    });
    console.log(banner);
    Notify.setBanner(banner);
  } catch (e) {
    const banner = colors.cyan("----- Spotify DL -----");
    console.log(banner);
    Notify.setBanner(banner);
  }
}

async function runCli() {
  console.clear();
  redrawBanner();

  // Set up redraw callback for when toast clears
  Notify.setRedrawCallback(() => {
    redrawBanner();
  });

  if (!process.env.SPOTIFY_CLIENT_ID || !process.env.SPOTIFY_CLIENT_SECRET) {
    console.error(
      colors.red(
        "\nError: SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET must be set in your .env file."
      )
    );
    console.error(
      colors.grey("Create a file named .env in the script directory with:")
    );
    console.error(colors.grey("SPOTIFY_CLIENT_ID=your_client_id"));
    console.error(colors.grey("SPOTIFY_CLIENT_SECRET=your_client_secret"));
    process.exit(1);
  }
  const credentials = {
    clientId: process.env.SPOTIFY_CLIENT_ID,
    clientSecret: process.env.SPOTIFY_CLIENT_SECRET,
  };

  const spotifyClient = new Spotify(credentials);
  const spotifyUserClient = new SpotifyUser(credentials);

  const initSpinner = ora(
    colors.cyan("Initializing Spotify connection...")
  ).start();
  try {
    await spotifyClient.initialize();
    initSpinner.succeed(colors.green("Spotify connection ready."));
  } catch (initError) {
    initSpinner.fail(
      colors.red("Failed to initialize Spotify client credentials.")
    );
    console.error(
      colors.red("Error details:"),
      initError.body || initError.message
    );
    console.error(
      colors.yellow(
        "Please check your SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET in the .env file."
      )
    );
    process.exit(1);
  }

  const md = new MusicDownloader(
    spotifyClient,
    spotifyUserClient,
    configManager
  );

  if (process.argv.length <= 2) {
    await runInteractiveMenu(md);
    return;
  }

  yargs(hideBin(process.argv))
    .scriptName(colors.magenta("spotify-dl"))
    .usage(
      `\n${colors.bold(
        "Spotify Music Downloader (Node.js)"
      )}\n\n${colors.underline("Usage:")} $0 <command> [options]`
    )
    .option("p", {
      alias: "path",
      describe: colors.yellow(
        "Override default download directory for this run"
      ),
      type: "string",
      normalize: true,
    })
    .command(
      "song <input>",
      colors.cyan("Download a single song from Spotify URL/URI/ID"),
      (y) =>
        y.positional("input", {
          describe: "Spotify track URL, URI, or ID",
          type: "string",
        }),
      async (argv) => {
        await md.downloadBySpotifyUri(argv.input, argv.path);
      }
    )
    .command(
      "album <input>",
      colors.cyan("Download songs from a Spotify Album URL/URI/ID"),
      (y) =>
        y.positional("input", {
          describe: "Spotify album URL, URI, or ID",
          type: "string",
        }),
      async (argv) => {
        await md.downloadBySpotifyUriAlbumMode(argv.input, argv.path);
      }
    )
    .command(
      "playlist <input>",
      colors.cyan("Download songs from a Spotify Playlist URL/URI/ID"),
      (y) =>
        y.positional("input", {
          describe: "Spotify playlist URL, URI, or ID",
          type: "string",
        }),
      async (argv) => {
        await md.downloadBySpotifyUriPlaylistMode(argv.input, argv.path);
      }
    )
    .command(
      "query <search_query>",
      colors.cyan("Search Spotify and download the first track result"),
      (y) =>
        y.positional("search_query", {
          describe: '"Artist - Title" or query',
          type: "string",
        }),
      async (argv) => {
        await md.downloadBySearchQuery(argv.search_query, argv.path);
      }
    )
    .command(
      "file <filename>",
      colors.cyan("Download Spotify track URIs/URLs/IDs listed in a file"),
      (y) =>
        y.positional("filename", {
          describe: "Path to the file (one input per line)",
          type: "string",
        }),
      async (argv) => {
        const filePath = path.resolve(argv.filename);
        console.log(colors.blue(`Reading URIs from file: ${filePath}`));
        try {
          const fileContent = await fs.readFile(filePath, "utf8");
          const uris = fileContent
            .split(/[\r\n]+/)
            .map((line) => line.trim())
            .filter((line) => line.length > 0 && !line.startsWith("#"));
          if (uris.length > 0) {
            await md.downloadBySpotifyUriList(uris, argv.path);
          } else {
            console.warn(
              colors.yellow(
                `File "${filePath}" is empty or contains no valid URIs (non-commented lines).`
              )
            );
          }
        } catch (fileError) {
          if (fileError.code === "ENOENT") {
            console.error(colors.red(`Error: File not found at "${filePath}"`));
          } else {
            console.error(
              colors.red(`Error reading file "${filePath}":`),
              fileError.message
            );
          }
          await Notify.send(
            `Failed to read input file: ${path.basename(filePath)}`,
            { error: true }
          );
        }
      }
    )
    .command(
      "settings",
      colors.yellow("Configure downloader settings"),
      async () => {
        console.log(colors.underline("\nCurrent Settings:"));
        console.log(
          ` - Download Path: ${colors.cyan(configManager.getDownloadPath())}`
        );
        console.log(
          ` - Config File: ${colors.grey(configManager.getConfigPath())}`
        );
        const answers = await inquirer.prompt([
          {
            type: "input",
            name: "newPath",
            message: "Enter new download path (leave blank to keep current):",
            default: configManager.getDownloadPath(),
            filter: (input) => input.trim(),
            validate: async (input) => {
              if (!input) return true;
              const absolutePath = path.resolve(input);
              try {
                await fs.access(path.dirname(absolutePath), fs.constants.W_OK);
                return true;
              } catch (err) {
                if (err.code === "ENOENT")
                  return `Parent directory does not exist: ${path.dirname(
                    absolutePath
                  )}`;
                if (err.code === "EACCES")
                  return `Permission denied for parent: ${path.dirname(
                    absolutePath
                  )}`;
                return `Invalid path: ${err.message}`;
              }
            },
          },
        ]);
        if (
          answers.newPath &&
          answers.newPath !== configManager.getDownloadPath()
        ) {
          await configManager.setDownloadPath(answers.newPath);
        } else {
          console.log(colors.grey("Download path unchanged."));
        }
      }
    )

    .demandCommand(1, colors.red("Please provide a command to execute."))
    .alias("h", "help")
    .alias("v", "version")
    .strict()
    .fail((msg, err, yargs) => {
      console.error(colors.red("\nError:", msg || err?.message || err));
      if (
        err?.message?.includes("yt-dlp") ||
        err?.message?.includes("youtube-dl")
      ) {
        console.error(
          colors.yellow(
            "\nHint: Ensure yt-dlp/youtube-dl and ffmpeg are installed and in PATH."
          )
        );
      } else if (err?.message?.includes("invalid_grant")) {
        console.error(
          colors.yellow(
            "\nHint: Spotify auth failed. Delete .spotify_refresh_token.secret.json and retry."
          )
        );
      }
      console.log("\n");
      yargs.showHelp();
      process.exit(1);
    })
    .wrap(null)
    .parse();
}

runCli().catch((error) => {
  console.error(colors.red("\nUnhandled error during execution:"), error);
  process.exit(1);
});
